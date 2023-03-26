#[cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{net::ToSocketAddrs, thread};
use anyhow::Result;
use smol::{channel, block_on};
use async_ssh2_lite::{AsyncSession};
use app::App;

use crate::ubus::{Ubus, UbusEvent};

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
    let (tx, rx) = channel::unbounded::<UbusEvent>();
    let listener = {
        let tx = tx.clone();
        tokio::spawn(async move {
            println!("before listen");
            if let Err(err) = ubus.listen(&[], tx).await {
                dbg!(err);
            };
            println!("after listen");
        })
    };

    loop {
        let e = rx.recv().await?;
        dbg!(e);
    }

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

