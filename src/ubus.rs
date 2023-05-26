use std::{borrow::Cow, str::FromStr, fmt::Display};

use anyhow::{anyhow, bail, Result};
use async_ssh2_lite::{AsyncIoTcpStream, AsyncSession};
use hex::FromHex;
use lazy_regex::regex_captures;
use serde_json::Value;
use shell_escape::unix::escape;
use smol::{channel::Sender, io::AsyncReadExt};
use thiserror::Error;

use crate::async_line_reader::AsyncLineReader;

type Session = AsyncSession<AsyncIoTcpStream>;

// TODO: Add tests

#[derive(Debug, Clone, Copy)]
pub enum UbusParamType {
    Unknown,
    Integer,
    Boolean,
    Table,
    String,
    Array,
    Double,
}

#[derive(Error, Debug)]
pub enum UbusError {
    #[error("Invalid command")]
    InvalidCommand,
    #[error("Invalid argument")]
    InvalidArgument,
    #[error("Method not found")]
    MethodNotFound,
    #[error("Not found")]
    NotFound,
    #[error("No response")]
    NoData,
    #[error("Permission denied")]
    PermissionDenied,
    #[error("Request timed out")]
    Timeout,
    #[error("Operation not supported")]
    NotSupported,
    #[error("Unknown error")]
    UnknownError,
    #[error("Connection failed")]
    ConnectionFailed,
    #[error("Out of memory")]
    NoMemory,
    #[error("Parsing message data failed")]
    ParseError,
    #[error("System error")]
    SystemError,
}

pub type Method = (String, Vec<(String, UbusParamType)>);

#[derive(Debug, Clone)]
pub struct Object {
    pub name: String,
    pub id: u32,
    pub methods: Vec<Method>,
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.id == other.id
    }
}

#[derive(Debug)]
pub struct ListenEvent {
    pub path: String,
    pub value: Value,
}

#[derive(Debug, Clone, Copy)]
pub enum MonitorEventType {
    Hello,
    Status,
    Data,
    Ping,
    Lookup,
    Invoke,
    AddObject,
    RemoveObject,
    Subscribe,
    Unsubscribe,
    Notify,
}

impl ToString for MonitorEventType {
    fn to_string(&self) -> String {
        use MonitorEventType::*;
        match self {
            Hello => "hello",
            Status => "status",
            Data => "data",
            Ping => "ping",
            Lookup => "lookup",
            Invoke => "invoke",
            AddObject => "add_object",
            RemoveObject => "remove_object",
            Subscribe => "subscribe",
            Unsubscribe => "unsubscribe",
            Notify => "notify",
        }
        .into()
    }
}

impl FromStr for MonitorEventType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        use MonitorEventType::*;
        match s {
            "hello" => Ok(Hello),
            "status" => Ok(Status),
            "data" => Ok(Data),
            "ping" => Ok(Ping),
            "lookup" => Ok(Lookup),
            "invoke" => Ok(Invoke),
            "add_object" => Ok(AddObject),
            "remove_object" => Ok(RemoveObject),
            "subscribe" => Ok(Subscribe),
            "unsubscribe" => Ok(Unsubscribe),
            "notify" => Ok(Notify),
            _ => Err(anyhow!("Unknown event type '{}'", s)),
        }
    }
}

impl Display for UbusParamType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use UbusParamType::*;
        let text = match self {
            String => "String",
            Boolean => "Boolean",
            Integer => "Integer",
            Double => "Double",
            Table => "Table",
            Array => "Array",
            Unknown => "(unknown)"
        };
        f.write_str(text)
    }
}

#[derive(Debug)]
pub struct MonitorEvent {
    direction: MonitorDir,
    client: u32,
    peer: u32,
    kind: MonitorEventType,
    data: Value, // TODO: Figure out the possible values for every `MonitorEventType`, to make this more safe.
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MonitorDir {
    Rx,
    Tx,
}

fn parse_parameter_type(param: &str) -> Option<UbusParamType> {
    use UbusParamType::*;
    match param {
        "String" => Some(String),
        "Boolean" => Some(Boolean),
        "Integer" => Some(Integer),
        "Double" => Some(Double),
        "Table" => Some(Table),
        "Array" => Some(Array),
        "(unknown)" => Some(Unknown),
        _ => None,
    }
}

fn parse_error_code(code: i32) -> Option<UbusError> {
    use UbusError::*;

    match code {
        0 => None,
        1 => Some(InvalidCommand),
        2 => Some(InvalidArgument),
        3 => Some(MethodNotFound),
        4 => Some(NotFound),
        5 => Some(NoData),
        6 => Some(PermissionDenied),
        7 => Some(Timeout),
        8 => Some(NotSupported),
        9 => Some(UnknownError),
        10 | -1 => Some(ConnectionFailed),
        11 => Some(NoMemory),
        12 => Some(ParseError),
        13 => Some(SystemError),
        _ => Some(UnknownError),
    }
}

fn parse_hex_id(id: &str) -> Result<u32> {
    let [byte1, byte2, byte3, byte4] = <[u8; 4]>::from_hex(id)?;
    let byte1 = (byte1 as u32) << 24;
    let byte2 = (byte2 as u32) << 16;
    let byte3 = (byte3 as u32) << 8;
    let byte4 = (byte4 as u32) << 0;

    return Ok(byte1 + byte2 + byte3 + byte4);
}

pub fn escape_json(json: &Value) -> String {
    escape(Cow::from(json.to_string())).into()
}

async fn exec_cmd(session: &Session, cmd: &str) -> Result<String> {
    let mut channel = session.channel_session().await?;
    channel.exec(cmd).await?;
    channel.send_eof().await?;
    channel.wait_eof().await?;
    channel.close().await?;
    channel.wait_close().await?;
    if let Some(err) = parse_error_code(channel.exit_status()?) {
        return Err(err.into());
    }

    let mut string_buffer = String::new();
    channel.read_to_string(&mut string_buffer).await?;
    Ok(string_buffer)
}

pub async fn list(session: &Session, path: Option<&str>) -> Result<Vec<String>> {
    let output = match path {
        Some(path) => exec_cmd(session, &format!("ubus -S list {}", path)).await?,
        None => exec_cmd(session, "ubus -S list").await?,
    };
    Ok(output.lines().map(ToOwned::to_owned).collect::<Vec<_>>())
}

pub async fn list_verbose(session: &Session, path: Option<&str>) -> Result<Vec<Object>> {
    let output = match path {
        Some(path) => exec_cmd(session, &format!("ubus -v list {}", path)).await?,
        None => exec_cmd(session, "ubus -v list").await?,
    };

    let mut cur_name = None;
    let mut cur_id = None;
    let mut cur_methods = vec![];

    let mut objects = vec![];
    for line in output.lines() {
        if let Some((_, name, id)) = regex_captures!(r"^'([\w\.-]+)' @([0-9a-zA-Z]{8})$", line) {
            if cur_name.is_some() && cur_id.is_some() {
                objects.push(Object {
                    id: cur_id.unwrap(),
                    name: cur_name.unwrap(),
                    methods: cur_methods,
                });
                cur_methods = vec![];
            }

            cur_name = Some(name.into());
            cur_id = Some(parse_hex_id(id)?);
        } else if let Some((_, name, params_body)) = regex_captures!(r#"^\s+"([\w\.-]+)":\{(.*)}$"#, line) {
            let mut params = vec![];
            if !params_body.is_empty() {
                for param in params_body.split(",") {
                    let (_, name, param_type_name) =
                        regex_captures!(r#"^"([\w-]+)":"([\w\(\)]+)"$"#, param).ok_or(anyhow!(
                            "Failed to parse parameter '{}' in line '{}'",
                            param,
                            line
                        ))?;

                    let param_type = parse_parameter_type(param_type_name)
                        .ok_or(anyhow!("Unknown parameter type '{}'", param_type_name))?;

                    params.push((name.into(), param_type));
                }
            }
            cur_methods.push((name.into(), params));
        } else {
            bail!("Failed to parse line '{}'", line);
        }
    }
    Ok(objects)
}

pub async fn call(
    session: &Session,
    path: &str,
    method: &str,
    message: Option<&Value>,
) -> Result<Value> {
    let cmd = match message {
        Some(msg) => format!("ubus -S call {} {} {}", path, method, escape_json(msg)),
        None => format!("ubus -S call {} {}", path, method),
    };

    // TODO: handle cases where output is empty? ("")
    let output = exec_cmd(session, &cmd).await?;
    if output.is_empty() {
        return Ok(Value::Null);
    }

    let value = serde_json::from_str::<Value>(&output)?;
    Ok(value)
}

pub async fn send(session: &Session, event_type: &str, message: Option<&Value>) -> Result<()> {
    let cmd = match message {
        Some(msg) => format!("ubus -S send {} {}", event_type, escape_json(msg)),
        None => format!("ubus -S send {}", event_type),
    };

    exec_cmd(session, &cmd).await?;

    Ok(())
}

pub async fn wait_for(session: &Session, objects: &[&str]) -> Result<()> {
    if objects.len() < 1 {
        bail!("At least 1 object is required")
    }
    let cmd = format!("ubus -S wait_for {}", objects.join(" "));
    let mut channel = session.channel_session().await?;
    channel.exec(&cmd).await?;
    channel.close().await?;

    Ok(())
}

fn parse_listen_event(bytes: &[u8]) -> Result<ListenEvent> {
    let event_value: Value = serde_json::from_slice(bytes)?;
    let event_map = event_value
        .as_object()
        .ok_or(anyhow!("Expected event to be an object"))?;

    if event_map.keys().len() != 1 {
        bail!("Expected event object to only contain one key");
    }

    let path = event_map.keys().next().unwrap().clone();
    let value = event_map.get(&path).unwrap().clone();

    Ok(ListenEvent { path, value })
}

pub async fn listen(session: &Session, paths: &[&str], sender: Sender<ListenEvent>) -> Result<()> {
    let cmd = format!("ubus -S listen {}", paths.join(" "));
    let mut channel = session.channel_session().await?;
    channel.exec(&cmd).await?;
    // TODO: Handle error? 'channel.exit_status()', idk if needed

    let mut line_reader = AsyncLineReader::new(channel.stream(0));
    loop {
        let line = line_reader.read_line().await?;
        let event = parse_listen_event(&line)?;
        sender.send(event).await?;
    }
}

pub async fn subscribe(session: &Session, paths: &[&str]) -> Result<()> {
    if paths.len() < 1 {
        bail!("At least 1 object is required")
    }
    let cmd = format!("ubus -S subscribe {}", paths.join(" "));
    let mut channel = session.channel_session().await?;
    channel.exec(&cmd).await?;

    // TODO: Haven't figured out how to test subscribe event using default objects on ubus.
    todo!();
}

fn parse_monitor_event(bytes: &[u8]) -> Result<MonitorEvent> {
    let line = bytes
        .iter()
        .map(|c| char::from_u32(*c as u32).unwrap())
        .collect::<String>();

    let (_, direction_str, client_str, peer_str, kind_str, data_str) = regex_captures!(
        r"^([<\->]{2}) ([0-9a-zA-Z]{8}) #([0-9a-zA-Z]{8})\s+([a-z_]+): (\{.*\})$",
        &line
    )
    .ok_or(anyhow!("Unknown pattern of monitor message '{}'", line))?;

    let direction = match direction_str {
        "->" => MonitorDir::Tx,
        "<-" => MonitorDir::Rx,
        _ => bail!("Unknown monitor message direction '{}'", direction_str),
    };
    let client = parse_hex_id(client_str)?;
    let peer = parse_hex_id(peer_str)?;
    let kind = MonitorEventType::from_str(kind_str)?;
    let data = serde_json::from_str(data_str)?;

    Ok(MonitorEvent {
        direction,
        client,
        peer,
        kind,
        data,
    })
}

pub async fn monitor(
    session: &Session,
    dir: Option<MonitorDir>,
    filter: &[MonitorEventType],
    sender: Sender<MonitorEvent>,
) -> Result<()> {
    let mut cmd = vec!["ubus -S".into()];
    if let Some(dir) = dir {
        if dir == MonitorDir::Rx {
            cmd.push("-M r".into());
        } else if dir == MonitorDir::Tx {
            cmd.push("-M t".into());
        }
    }
    cmd.extend(filter.iter().map(|e| format!("-m {}", e.to_string())));
    cmd.push("monitor".into());

    let mut channel = session.channel_session().await?;
    println!("{}", cmd.join(" "));
    channel.exec(&cmd.join(" ")).await?;
    // TODO: Handle error? 'channel.exit_status()', idk if needed

    let mut line_reader = AsyncLineReader::new(channel.stream(0));
    loop {
        let line = line_reader.read_line().await?;
        let event = parse_monitor_event(&line)?;
        sender.send(event).await?;
    }
}
