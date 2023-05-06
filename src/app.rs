use std::{
    net::{SocketAddr, SocketAddrV4},
    sync::{mpsc::{Receiver, Sender}, Arc},
    vec, rc::Rc,
};

use anyhow::Result;
use async_ssh2_lite::{AsyncIoTcpStream, AsyncSession};
use eframe::CreationContext;
use egui::{text::LayoutJob, Color32};
use lazy_regex::regex_replace_all;
use serde_json::Value;

use crate::ubus;

const ERROR_COLOR: Color32 = Color32::from_rgb(180, 20, 20);
const SUCCESS_COLOR: Color32 = Color32::from_rgb(20, 150, 20);

pub enum AsyncEvent {
    Connect(Result<AsyncSession<AsyncIoTcpStream>>),
    Disconnect(Result<()>),
    ListObjects(Result<Vec<ubus::Object>>),
    Call(Result<Value>)
}

pub struct AppSettings {
    address: String,
    port: u16,
    username: String,
    password: String,

    show_object_ids: bool,
    connect_immidiately: bool
}

pub struct App {
    settings: AppSettings,
    session: Option<AsyncSession<AsyncIoTcpStream>>,

    selected_object: Option<Rc<ubus::Object>>,
    selected_method: Option<String>,
    object_filter: String,
    objects: Vec<Rc<ubus::Object>>,
    payload: String,
    response: Option<Result<Value>>,

    is_connecting: bool,
    is_disconnecting: bool,

    tx: Sender<AsyncEvent>,
    rx: Receiver<AsyncEvent>,
}

impl Default for App {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();

        Self {
            settings: AppSettings {
                address: "172.24.224.1".into(), //"192.168.1.1".to_owned(),
                port: 22,
                username: "root".to_owned(),
                password: "admin01".to_owned(),
                show_object_ids: false,
                connect_immidiately: true
            },
            session: None,

            object_filter: "".into(),
            selected_object: None,
            selected_method: None,
            payload: "".into(),
            response: None,
            objects: vec![],

            is_connecting: false,
            is_disconnecting: false,

            tx,
            rx,
        }
    }
}

async fn connect<A>(
    socket_addr: A,
    username: String,
    password: String,
) -> Result<AsyncSession<AsyncIoTcpStream>>
where
    A: Into<SocketAddr>,
{
    let mut session = AsyncSession::<AsyncIoTcpStream>::connect(socket_addr, None).await?;
    session.handshake().await?;
    session.userauth_password(&username, &password).await?;
    return Ok(session);
}

async fn disconnect(session: AsyncSession<AsyncIoTcpStream>) -> Result<()> {
    session
        .disconnect(
            Some(ssh2::DisconnectCode::ByApplication),
            "Disconnect",
            Some("en"),
        )
        .await?;
    Ok(())
}

fn remove_json_comments(text: &str) -> String {
    let text = regex_replace_all!(r#"/\*(.|\n)*?\*/"#, &text, |_, _| ""); // Multi line comments
    let text = regex_replace_all!(r#"//.*\n?"#, &text, |_| ""); // Single line comments
    let text = regex_replace_all!(r#"(,)(\s*[\]}])"#, &text, |_, _, rest: &str| rest.to_string()); // Trailing commas

    text.into()
}


fn json_layouter(ui: &egui::Ui, string: &str, wrap_width: f32) -> Arc<egui::Galley> {
    let mut layout_job = crate::syntax_highlighting::highlight(ui.ctx(), string, "json");
    layout_job.wrap.max_width = wrap_width;
    ui.fonts(|f| f.layout_job(layout_job))
}

impl App {
    pub fn init(&mut self, _cc: &CreationContext) {
        if self.settings.connect_immidiately {
            let username = &self.settings.username;
            let password = &self.settings.password;
            if username.is_empty() || password.is_empty() {
                return;
            }

            let address = self.settings.address.parse();
            if address.is_err() {
                return;
            }
            let address = address.unwrap();
            let port = self.settings.port;

            let socket_addr = SocketAddrV4::new(address, port);
            self.start_connect(socket_addr, username.clone(), password.clone());
        }
    }

    fn handle_events(&mut self, _ctx: &egui::Context) {
        use AsyncEvent::*;

        if let Ok(event) = self.rx.try_recv() {
            match event {
                Connect(result) => {
                    self.is_connecting = false;
                    match result {
                        Ok(session) => {
                            self.session = Some(session);
                            self.start_list_objects()
                        }
                        Err(err) => todo!("{}", err),
                    }
                }

                Disconnect(result) => {
                    self.is_disconnecting = false;

                    if let Err(err) = result {
                        todo!("{}", err)
                    }
                }

                ListObjects(result) => match result {
                    Ok(objects) => self.objects = objects.into_iter().map(Rc::new).collect(),
                    Err(err) => todo!("{}", err),
                },

                Call(result) => {
                    self.response = Some(result)
                }
            }
        }
    }

    fn start_connect<A>(&mut self, socket_addr: A, username: String, password: String)
    where
        A: Into<SocketAddr>,
    {
        if self.session.is_some() {
            return;
        }

        self.is_connecting = true;
        let tx = self.tx.clone();
        let socket_addr = socket_addr.into();
        tokio::spawn(async move {
            let result = connect(socket_addr, username, password).await;
            tx.send(AsyncEvent::Connect(result))
                .expect("Failed to send event");
        });
    }

    fn start_disconnect(&mut self) {
        if self.session.is_none() {
            return;
        }

        self.is_disconnecting = true;
        let tx = self.tx.clone();
        let session = self.session.clone().unwrap();
        self.session = None;
        tokio::spawn(async move {
            let result = disconnect(session).await;
            tx.send(AsyncEvent::Disconnect(result))
                .expect("Failed to send event");
        });
    }

    fn start_call(&mut self, object: String, method: String, message: Option<Value>) {
        if self.session.is_none() {
            return;
        }

        let tx = self.tx.clone();
        let session = self.session.clone().unwrap();
        tokio::spawn(async move {
            let result = ubus::call(&session, &object, &method, message.as_ref()).await;
            tx.send(AsyncEvent::Call(result)).expect("Failed to send event");
        });
    }

    fn start_list_objects(&self) {
        if self.session.is_none() {
            return;
        }
        let session = self.session.clone().unwrap();

        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = ubus::list_verbose(&session, None).await;
            tx.send(AsyncEvent::ListObjects(result))
                .expect("Failed to send event");
        });
    }

    fn show_left_panel(&mut self, ui: &mut egui::Ui) {
        use egui::*;

        ui.text_edit_singleline(&mut self.object_filter);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for obj in self.objects.iter() {
                let shown_methods;
                if obj.name.contains(&self.object_filter) {
                    shown_methods = obj.methods.iter()
                        .by_ref()
                        .collect::<Vec<_>>();
                } else {
                    shown_methods = obj.methods.iter()
                        .filter(|(name, _)| name.contains(&self.object_filter))
                        .collect::<Vec<_>>();
                }

                if shown_methods.len() > 0 {
                    let style = ui.style();
                    let mut text = LayoutJob::default();
                    text.append(&obj.name, 0.0, TextFormat {
                        ..TextFormat::default()
                    });
                    if self.settings.show_object_ids {
                        text.append(&format!("@{:x}", obj.id), 10.0, TextFormat {
                            color: style.noninteractive().fg_stroke.color,
                            italics: true,
                            ..TextFormat::default()
                        });
                    }
                    CollapsingHeader::new(text).show(ui, |ui| {
                        for (name, method_params) in &shown_methods {
                            if ui.selectable_label(false, name).clicked() {
                                self.selected_object = Some(obj.clone());
                                self.selected_method = Some(name.clone());
                                let method_params = method_params.iter().map(|(s, p)| (s.as_str(), *p)).collect::<Vec<_>>();
                                self.payload = App::create_default_payload(&method_params);
                            };
                        }
                    });
                }
            }
        });
    }

    fn show_right_panel(&mut self, ui: &mut egui::Ui) {
        use egui::*;

        egui::ScrollArea::vertical()
            .show(ui, |ui| {
            ui.text_edit_singleline(&mut self.settings.address);
            // TODO: ui.text_edit_singleline(&mut self.port);
            ui.text_edit_singleline(&mut self.settings.username);
            ui.text_edit_singleline(&mut self.settings.password);
            if self.is_connecting {
                ui.add_enabled(false, Button::new("Connecting..."));
            } else if self.session.is_none() {
                if ui.button("Connect").clicked() {
                    let socket_addr = SocketAddrV4::new(self.settings.address.parse().unwrap(), self.settings.port);
                    self.start_connect(socket_addr, self.settings.username.clone(), self.settings.password.clone());
                }
            } else {
                if ui.button("Disconnect").clicked() {
                    self.start_disconnect()
                }
            }
        });
    }

    fn create_default_payload(params: &[(&str, ubus::UbusParamType)]) -> String {
        let mut lines = vec![];
        for (param_name, param_type) in params {
            use ubus::UbusParamType::*;
            let param_value = match param_type {
                Unknown => "\"<unknown>\"",
                Integer => "0",
                Boolean => "false",
                Table => "{}",
                String => "\"\"",
                Array => "[]",
                Double => "0.00",
            };
            lines.push(format!("\t\"{}\": {}, // {}", &param_name, &param_value, param_type));
        }

        return format!("{{\n{}\n}}", lines.join("\n"));
    }

    fn display_response_textbox(ui: &mut egui::Ui, response: &Result<Value>) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut text = match response {
                Ok(Value::Null) => "Success!".into(),
                Ok(response) => serde_json::to_string_pretty(response).unwrap(),
                Err(err) => format!("Error: {}", err)
            };

            let textbox = egui::TextEdit::multiline(&mut text)
                .font(egui::TextStyle::Monospace) // for cursor height
                .code_editor()
                .desired_rows(4)
                .lock_focus(true)
                .desired_width(f32::INFINITY);

            match response {
                Ok(Value::Null) => {
                    ui.add(textbox.text_color(SUCCESS_COLOR));
                },
                Err(_) => {
                    ui.add(textbox.text_color(ERROR_COLOR));
                },
                Ok(_) => {
                    ui.add(textbox.layouter(&mut json_layouter));
                }
            };
        });
    }

    fn show_central_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Object dropdown
            {
                let object_name = self.selected_object.as_ref().map(|obj| obj.name.as_ref()).unwrap_or("");
                let object_combobox = egui::ComboBox::from_id_source("selected_object")
                    .selected_text(object_name)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        let mut selection = None;
                        for object in &self.objects {
                            ui.selectable_value(&mut selection, Some(object.clone()), &object.name);
                        }
                        return selection;
                    });

                match object_combobox.inner {
                    Some(Some(object)) => {
                        self.selected_method = None;
                        self.selected_object = Some(object);
                    },
                    _ => {}
                };
            }

            // Method dropdown
            {
                let selected_method_name = self.selected_method.as_deref().unwrap_or("");
                let method_combobox = ui.add_enabled_ui(self.selected_object.is_some(), |ui| {
                    let mut selection = None;
                    egui::ComboBox::from_id_source("selected_method")
                        .selected_text(selected_method_name)
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            if let Some(object) = &self.selected_object {
                                for method in &object.methods {
                                    let method_name = &method.0;
                                    let mut label_response = ui.selectable_label(selected_method_name == method_name, method_name);
                                    if label_response.clicked() && selected_method_name != method_name {
                                        selection = Some(method_name.clone());
                                        label_response.mark_changed();
                                    }
                                }
                            }
                        });
                    return selection;
                });

                match (method_combobox.inner, &self.selected_object) {
                    (Some(method), Some(object)) => {
                        let method_params = object.methods.iter()
                            .find(|(name, _)| name.eq(&method))
                            .map(|(_, params)| params)
                            .unwrap()
                            .iter()
                            .map(|(param_name, param_type)| (param_name.as_str(), *param_type))
                            .collect::<Vec<_>>();
                        self.payload = App::create_default_payload(&method_params);
                        self.selected_method = Some(method);
                    },
                    _ => {}
                };
            }

            let call_enabled = self.selected_object.is_some() && self.selected_method.is_some();
            ui.add_enabled_ui(call_enabled, |ui| {
                if ui.button("call").clicked() {
                    let object_name = self.selected_object.as_ref().unwrap().name.clone();
                    let method_name = self.selected_method.as_ref().unwrap().clone();
                    let payload = remove_json_comments(&self.payload);
                    let message = serde_json::from_str(&payload).unwrap(); // TODO: handle parsing error
                    self.start_call(object_name, method_name, Some(message));
                    // TODO: Block sending other requests
                }
            });
        });

        ui.separator();

        if let Some(response) = &self.response {
            egui::TopBottomPanel::bottom("bottom_panel")
                .resizable(true)
                .show_inside(ui, |ui| {
                    ui.add_space(10.0);
                    App::display_response_textbox(ui, response);
                });
        }

        egui::CentralPanel::default()
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.payload)
                            .font(egui::TextStyle::Monospace) // for cursor height
                            .code_editor()
                            .desired_rows(10)
                            .lock_focus(true)
                            .desired_width(f32::INFINITY)
                            .layouter(&mut json_layouter),
                    );
                });
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::SidePanel::left("left_panel")
                .resizable(true)
                .width_range(100.0..=300.0)
                .show_inside(ui, |ui| self.show_left_panel(ui));

            egui::SidePanel::right("right_panel")
                .resizable(true)
                .width_range(100.0..=200.0)
                .show_inside(ui, |ui| self.show_right_panel(ui));

            egui::CentralPanel::default()
                .show_inside(ui, |ui| self.show_central_panel(ui));
        });
    }
}
