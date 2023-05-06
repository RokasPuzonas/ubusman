use std::{
    net::{SocketAddr, SocketAddrV4},
    sync::{mpsc::{Receiver, Sender}, Arc},
    vec, rc::Rc, time::SystemTime, path::{PathBuf, Path}, fs,
};

use anyhow::{Result, anyhow};
use async_ssh2_lite::{AsyncIoTcpStream, AsyncSession};
use directories_next::ProjectDirs;
use eframe::CreationContext;
use egui::{text::LayoutJob, Color32, ColorImage, TextureHandle};
use lazy_regex::regex_replace_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use lazy_static::lazy_static;
use tokio::task::JoinHandle;
use git_version::git_version;
use version::version;

use crate::ubus::{self, escape_json};

const ERROR_COLOR: Color32 = Color32::from_rgb(180, 20, 20);
const SUCCESS_COLOR: Color32 = Color32::from_rgb(20, 150, 20);
lazy_static! {
    pub static ref COPY_ICON: ColorImage = load_image_from_memory(include_bytes!("./copy.png"))
        .expect("Failed to load copy icon") as ColorImage;
}

const SOURCE_CODE_URL: &str = "https://git.rpuzonas.com/rpuzonas/ubusman";
const VERSION: &str = version!();
const GIT_VERSION: &str = git_version!(args = ["--always", "--dirty=*"]);

pub fn load_image_from_memory(image_data: &[u8]) -> Result<ColorImage, image::ImageError> {
    let image = image::load_from_memory(image_data)?;
    let size = [image.width() as _, image.height() as _];
    let image_buffer = image.to_rgba8();
    let pixels = image_buffer.as_flat_samples();
    Ok(ColorImage::from_rgba_unmultiplied(
        size,
        pixels.as_slice(),
    ))
}

pub enum AsyncEvent {
    Connect(Result<AsyncSession<AsyncIoTcpStream>>),
    Disconnect(Result<()>),
    ListObjects(Result<Vec<ubus::Object>>),
    Call(Result<Value>)
}

#[derive(Deserialize, Serialize)]
pub struct AppSettings {
    address: String,
    port: u16,
    username: String,
    password: String,

    show_object_ids: bool,
    connect_on_start: bool
}

pub struct App {
    settings: AppSettings,
    session: Option<AsyncSession<AsyncIoTcpStream>>,
    ubus_call_handle: Option<JoinHandle<()>>,
    last_ubus_call_at: SystemTime,

    selected_object: Option<Rc<ubus::Object>>,
    selected_method: Option<String>,
    object_filter: String,
    objects: Vec<Rc<ubus::Object>>,
    payload: String,
    response: Option<Result<Value>>,

    is_connecting: bool,
    is_disconnecting: bool,
    settings_open: bool,

    tx: Sender<AsyncEvent>,
    rx: Receiver<AsyncEvent>,

    copy_texture: Option<TextureHandle>
}

fn get_config_path() -> PathBuf {
    let project_dirs = ProjectDirs::from("", "",  "ubusman")
        .expect("Failed to determine home directory");
    let config_dir = project_dirs.config_dir();
    config_dir.join("config.toml")
}

impl Default for App {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();

        Self {
            settings: AppSettings {
                address: "192.168.1.1".to_owned(),
                port: 22,
                username: "root".to_owned(),
                password: "admin01".to_owned(),
                show_object_ids: false,
                connect_on_start: true
            },
            session: None,
            ubus_call_handle: None,
            last_ubus_call_at: SystemTime::UNIX_EPOCH,

            object_filter: "".into(),
            selected_object: None,
            selected_method: None,
            payload: "".into(),
            response: None,
            objects: vec![],

            is_connecting: false,
            is_disconnecting: false,
            settings_open: false,

            tx,
            rx,

            copy_texture: None
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
    pub fn init(&mut self, cc: &CreationContext) {
        if self.settings.connect_on_start {
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

        self.copy_texture = Some(cc.egui_ctx.load_texture(
            "clipboard",
            COPY_ICON.clone(),
            Default::default()
        ));

        self.load_config();
    }

    fn load_config(&mut self) -> Result<()> {
        let config_path = get_config_path();
        if let Ok(contents) = fs::read_to_string(config_path) {
            self.settings = toml::from_str(&contents)?;
        }

        Ok(())
    }

    fn save_config(&self) -> Result<()> {
        let config_path = get_config_path();
        let directory = Path::parent(&config_path)
            .expect("Failed to get config parent directory");
        if !Path::is_dir(directory) {
            fs::create_dir_all(directory)?;
        }

        fs::write(&config_path, toml::to_string_pretty(&self.settings)?)?;
        Ok(())
    }

    pub fn get_selected_object(&self) -> Option<&str> {
        self.selected_object.as_ref().map(|obj| obj.name.as_str())
    }

    pub fn get_selected_method(&self) -> Option<&str> {
        self.selected_method.as_ref().map(|method| method.as_str())
    }

    pub fn get_payload(&self) -> serde_json::Result<Value> {
        let stripped_payload = remove_json_comments(&self.payload);
        serde_json::from_str(&stripped_payload)
    }

    pub fn is_ubus_call_in_progress(&self) -> bool {
        if let Some(handle) = &self.ubus_call_handle {
            return !handle.is_finished();
        }
        return false;
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
                    self.response = Some(result);
                    self.ubus_call_handle = None;
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
        let handle = tokio::spawn(async move {
            let result = ubus::call(&session, &object, &method, message.as_ref()).await;
            tx.send(AsyncEvent::Call(result)).expect("Failed to send event");
        });
        self.ubus_call_handle = Some(handle);
        self.last_ubus_call_at = SystemTime::now()
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

        let mut match_exists = false;
        ui.text_edit_singleline(&mut self.object_filter);
        ui.add_space(8.0);
        ScrollArea::vertical().show(ui, |ui| {
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
                    match_exists = true;
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

        if !match_exists {
            ui.label("no matches :(");
        }
    }

    fn show_settings(ui: &mut egui::Ui, settings: &mut AppSettings) {
        ui.checkbox(&mut settings.show_object_ids, "Show object IDs");
        ui.checkbox(&mut settings.connect_on_start, "Connect on start");
        ui.add_space(16.0);
        ui.label(format!("Version: {}-{}", VERSION, GIT_VERSION));
        ui.label("Author: Rokas Puzonas");
        if ui.link("Source code").clicked() {
            ui.output_mut(|o| o.open_url(SOURCE_CODE_URL));
        }
    }

    fn show_right_panel(&mut self, ui: &mut egui::Ui) {
        use egui::*;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("Address:");
            ui.text_edit_singleline(&mut self.settings.address);
            // TODO: ui.text_edit_singleline(&mut self.port);
            ui.label("Username:");
            ui.text_edit_singleline(&mut self.settings.username);
            ui.label("Password:");
            ui.text_edit_singleline(&mut self.settings.password);
            ui.add_space(8.0);
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

            if ui.button("Settings").clicked() {
                self.settings_open = !self.settings_open;
            }
            let window = egui::Window::new("Settings")
                .resizable(false)
                .collapsible(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .open(&mut self.settings_open);
            window.show(ui.ctx(), |ui| App::show_settings(ui, &mut self.settings));

            ui.add_space(32.0);
            ui.label("TODO: Add saving calls");
            ui.label("TODO: Add listen logs");
            ui.label("TODO: Add monitor logs");
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
        use egui::*;
        let time_since_last_call = self.last_ubus_call_at.elapsed().expect("Failed to get time since last ubus call");
        let call_in_progress = self.is_ubus_call_in_progress() && time_since_last_call.as_millis() >= 100;
        let call_enabled = self.selected_object.is_some() && self.selected_method.is_some();

        ui.horizontal(|ui| {
            ui.add_enabled_ui(!call_in_progress, |ui| {
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

                if ui.add_enabled(call_enabled, Button::new("call")).clicked() {
                    let object_name = self.get_selected_object().unwrap().into();
                    let method_name = self.get_selected_method().unwrap().into();
                    let payload = self.get_payload().unwrap(); // TODO: handle parsing error
                    self.start_call(object_name, method_name, Some(payload));
                    // TODO: Block sending other requests
                }
            });

            let copy_icon = self.copy_texture.as_ref().expect("Copy icon not loaded");
            let copy_button = Button::image_and_text(copy_icon.id(), Vec2::new(10.0, 10.0), "copy");
            if ui.add_enabled(call_enabled, copy_button).clicked() {
                let object_name = self.get_selected_object().unwrap();
                let method_name = self.get_selected_method().unwrap();
                let payload = self.get_payload().unwrap(); // TODO: handle parsing error
                let cmd = format!("ubus call {} {} {}", object_name, method_name, escape_json(&payload));
                ui.output_mut(|o| o.copied_text = cmd);
            }

            if call_in_progress {
                ui.spinner();
            }
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
                    ui.add_enabled(
                        !call_in_progress,
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
                .width_range(125.0..=200.0)
                .show_inside(ui, |ui| self.show_right_panel(ui));

            egui::CentralPanel::default()
                .show_inside(ui, |ui| self.show_central_panel(ui));
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_config();
    }
}
