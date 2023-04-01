use std::{
    default,
    net::{SocketAddr, SocketAddrV4, ToSocketAddrs},
    sync::mpsc::{Receiver, Sender},
};

use anyhow::{Error, Result};
use async_ssh2_lite::{AsyncIoTcpStream, AsyncSession};
use eframe::CreationContext;
use serde::de::IntoDeserializer;

pub enum AsyncEvent {
    Connect(Result<AsyncSession<AsyncIoTcpStream>>),
    Disconnect(Result<()>)
}

pub struct App {
    address: String,
    port: u16,
    username: String,
    password: String,
    session: Option<AsyncSession<AsyncIoTcpStream>>,

    is_connecting: bool,
    is_disconnecting: bool,

    tx: Sender<AsyncEvent>,
    rx: Receiver<AsyncEvent>,
}

impl Default for App {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();

        Self {
            address: "172.24.224.1".into(), //"192.168.1.1".to_owned(),
            port: 22,
            username: "root".to_owned(),
            password: "admin01".to_owned(),
            session: None,

            is_connecting: false,
            is_disconnecting: false,

            tx,
            rx,
        }
    }
}

async fn connect<A>(socket_addr: A, username: String, password: String) -> Result<AsyncSession<AsyncIoTcpStream>>
where
    A: Into<SocketAddr>,
{
    let mut session = AsyncSession::<AsyncIoTcpStream>::connect(socket_addr, None).await?;
    session.handshake().await?;
    session.userauth_password(&username, &password).await?;
    return Ok(session);
}

async fn disconnect(session: AsyncSession<AsyncIoTcpStream>) -> Result<()> {
    session.disconnect(Some(ssh2::DisconnectCode::ByApplication), "Disconnect", Some("en")).await?;
    Ok(())
}

impl App {
    pub fn init(&mut self, _cc: &CreationContext) {}

    fn handle_connect_event(&mut self, result: Result<AsyncSession<AsyncIoTcpStream>>) {
        self.is_connecting = false;
        match result {
            Ok(session) => {
                self.session = Some(session)
            },
            Err(_) => todo!(),
        }
    }

    fn handle_disconnect_event(&mut self, result: Result<()>) {
        self.is_disconnecting = false;

        if let Err(err) = result {
            todo!()
        }
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        if let Ok(event) = self.rx.try_recv() {
            match event {
                AsyncEvent::Connect(result) => self.handle_connect_event(result),
                AsyncEvent::Disconnect(result) => self.handle_disconnect_event(result),
            }
        }
    }

    fn start_connect<A>(&mut self, socket_addr: A, username: String, password: String)
    where
        A: Into<SocketAddr>,
    {
        if self.session.is_some() { return; }

        self.is_connecting = true;
        let tx = self.tx.clone();
        let socket_addr = socket_addr.into();
        tokio::spawn(async move {
            let result = connect(socket_addr, username, password).await;
            tx.send(AsyncEvent::Connect(result)).expect("Failed to send event");
        });
    }

    fn start_disconnect(&mut self) {
        if self.session.is_none() { return; }

        self.is_disconnecting = true;
        let tx = self.tx.clone();
        let session = self.session.clone().unwrap();
        self.session = None;
        tokio::spawn(async move {
            let result = disconnect(session).await;
            tx.send(AsyncEvent::Disconnect(result)).expect("Failed to send event");
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        use egui::*;

        self.handle_events(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::SidePanel::left("left_panel")
                .resizable(true)
                .default_width(150.0)
                .width_range(80.0..=200.0)
                .show_inside(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading("Left Panel");
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label("Foo");
                    });
                });

            egui::SidePanel::right("right_panel")
                .resizable(true)
                .default_width(150.0)
                .width_range(80.0..=200.0)
                .show_inside(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading("Right Panel");
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.text_edit_singleline(&mut self.address);
                        // TODO: ui.text_edit_singleline(&mut self.port);
                        ui.text_edit_singleline(&mut self.username);
                        ui.text_edit_singleline(&mut self.password);
                        if self.is_connecting {
                            ui.button("Connecting...");
                        } else if self.session.is_none() {
                            if ui.button("Connect").clicked() {
                                let socket_addr = SocketAddrV4::new(self.address.parse().unwrap(), self.port);
                                self.start_connect(socket_addr, self.username.clone(), self.password.clone());
                            }
                        } else {
                            if ui.button("Disconnect").clicked() {
                                self.start_disconnect()
                            }
                        }
                    });
                });

            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Central Panel");
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label("foo");
                });
            });
        });
    }
}
