use std::net::ToSocketAddrs;

#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use anyhow::Result;
use smol::Async;
use async_ssh2_lite::{AsyncSession, AsyncSessionStream};
use app::App;

use crate::ubus::Ubus;

mod app;
mod ubus;

#[tokio::main]
async fn main() -> Result<()> {
    let address = "172.24.224.1:22";
    let username = "root";
    let password = "admin01";

    let mut session = AsyncSession::<async_ssh2_lite::AsyncIoTcpStream>::connect(
        address.to_socket_addrs()?.next().unwrap(),
        None,
    ).await?;

    session.handshake().await?;
    session.userauth_password(username, password).await?;

    let ubus = Ubus::new(session);
    // dbg!(ubus.call(
    //     "container",
    //     "get_features",
    //     None
    //     ).await?
    // );

    dbg!(ubus.wait_for(&vec!["network"]).await?);

    /*
    let mut native_options = eframe::NativeOptions::default();
    native_options.decorated = true;
    native_options.resizable = true;
    let mut app = App::default();

    eframe::run_native(
        "ubusman",
        native_options,
        Box::new(move |cc| {
            app.init(cc);
            Box::new(app)
        })
    )
    */

    Ok(())
}

