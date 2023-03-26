use std::{io::Read, borrow::Cow, net::TcpStream};

use async_ssh2_lite::AsyncSession;
use lazy_regex::regex_captures;
use serde_json::{Value, json};
use shell_escape::unix::escape;
use hex::FromHex;
use anyhow::{Result, bail, anyhow};
use smol::{Async, io::AsyncReadExt, channel::Sender};
use thiserror::Error;

pub struct Ubus {
    session: AsyncSession<Async<TcpStream>>
}

#[derive(Debug)]
pub enum UbusParamType {
    Unknown,
    Integer,
    Boolean,
    Table,
    String,
    Array,
    Double
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

#[derive(Debug)]
pub struct UbusObject {
    pub name: String,
    pub id: u32,
    pub methods: Vec<(String, Vec<(String, UbusParamType)>)>
}

#[derive(Debug)]
pub struct UbusEvent {
    pub path: String,
    pub value: Value
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
        _ => None
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
        10 => Some(ConnectionFailed),
        11 => Some(NoMemory),
        12 => Some(ParseError),
        13 => Some(SystemError),
        _ => Some(UnknownError)
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

fn escape_json(json: &Value) -> String {
    escape(Cow::from(json.to_string())).into()
}

impl Ubus {
    pub fn new(session: AsyncSession<Async<TcpStream>>) -> Ubus {
        Ubus {
            session
        }
    }

    async fn exec_cmd(&self, cmd: &str) -> Result<String> {
        let mut channel = self.session.channel_session().await?;
        channel.exec(cmd).await?;
        channel.close().await?;
        if let Some(err) = parse_error_code(channel.exit_status()?) {
            return Err(err.into())
        }

        let mut output = String::new();
        channel.read_to_string(&mut output).await?;
        Ok(output)
    }

    pub async fn list(self, path: Option<&str>) -> Result<Vec<String>> {
        let output = match path {
            Some(path) => self.exec_cmd(&format!("ubus list {}", path)).await?,
            None => self.exec_cmd("ubus list").await?,
        };
        Ok(output.lines().map(ToOwned::to_owned).collect::<Vec<_>>())
    }

    pub async fn list_verbose(self, path: Option<&str>) -> Result<Vec<UbusObject>> {
        let output = match path {
            Some(path) => self.exec_cmd(&format!("ubus -v list {}", path)).await?,
            None => self.exec_cmd("ubus -v list").await?,
        };

        let mut cur_name = None;
        let mut cur_id = None;
        let mut cur_methods = vec![];

        let mut objects = vec![];
        for line in output.lines() {
            if let Some((_, name, id)) = regex_captures!(r"^'([\w.-]+)' @([0-9a-zA-Z]+)$", line) {
                if cur_name.is_some() && cur_id.is_some() {
                    objects.push(UbusObject {
                        id: cur_id.unwrap(),
                        name: cur_name.unwrap(),
                        methods: cur_methods
                    });
                    cur_methods = vec![];
                }

                cur_name = Some(name.into());
                cur_id = Some(parse_hex_id(id)?);
            } else if let Some((_, name, params_body)) = regex_captures!(r#"^\s+"([\w-]+)":\{(.*)}$"#, line) {
                let mut params = vec![];
                if !params_body.is_empty() {
                    for param in params_body.split(",") {
                        let (_, name, param_type_name) = regex_captures!(r#"^"([\w-]+)":"(\w+)"$"#, param)
                            .ok_or(anyhow!("Failed to parse parameter '{}' in line '{}'", param, line))?;

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

    pub async fn call(self, path: &str, method: &str, message: Option<&Value>) -> Result<Value> {
        let cmd = match message {
            Some(msg) => format!("ubus call {} {} {}", path, method, escape_json(msg)),
            None => format!("ubus call {} {}", path, method),
        };

        let output = self.exec_cmd(&cmd).await?;
        let value = serde_json::from_str::<Value>(&output)?;
        Ok(value)
    }

    pub async fn send(self, event_type: &str, message: Option<&Value>) -> Result<()> {
        let cmd = match message {
            Some(msg) => format!("ubus send {} {}", event_type, escape_json(msg)),
            None => format!("ubus send {}", event_type),
        };

        self.exec_cmd(&cmd).await?;

        Ok(())
    }

    pub async fn wait_for(self, objects: &[&str]) -> Result<()> {
        if objects.len() < 1 {
            bail!("At least 1 object is required")
        }
        let cmd = format!("ubus wait_for {}", objects.join(" "));
        let mut channel = self.session.channel_session().await?;
        channel.exec(&cmd).await?;
        channel.close().await?;

        Ok(())
    }

    fn parse_event(bytes: &[u8]) -> Result<UbusEvent> {
        let event_value: Value = serde_json::from_slice(bytes)?;
        let event_map = event_value.as_object()
            .ok_or(anyhow!("Expected event to be an object"))?;

        if event_map.keys().len() != 1 {
            bail!("Expected event object to only contain one key");
        }

        let path = event_map.keys().next().unwrap().clone();
        let value = event_map.get(&path).unwrap().clone();

        Ok(UbusEvent { path, value })
    }

    pub async fn listen(self, paths: &[&str], sender: Sender<UbusEvent>) -> Result<()> {
        let cmd = format!("ubus listen {}", paths.join(" "));
        let mut channel = self.session.channel_session().await?;
        channel.exec(&cmd).await?;

        let mut line_buffer = vec![0u8; 1024];
        let mut buffer_size = 0usize;
        loop {
            let n = channel.read(&mut line_buffer[buffer_size..]).await?;

            let delim_pos = line_buffer[buffer_size..(buffer_size+n)]
                .iter()
                .position(|c| *c == b'\n')
                .map(|pos| pos + buffer_size);

            buffer_size += n;
            if let Some(pos) = delim_pos {
                let event = Ubus::parse_event(&line_buffer[0..pos])?;
                sender.send(event).await?;

                line_buffer.copy_within((pos+1)..buffer_size, 0);
                buffer_size -= pos;
                buffer_size -= 1;
            }


            // Double line buffer size if at capacity
            if buffer_size == line_buffer.len() {
                for _ in 0..=line_buffer.len() {
                    line_buffer.push(0u8);
                }
            }
        }
    }
}
