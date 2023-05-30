use std::net::SocketAddr;
use std::time::Duration;

use anyhow::anyhow;
use clap::Parser;
use log::{error, info};
use tokio::select;
use tokio::sync::watch;

use crate::child::LinuxChild;
use crate::proxy::TCPEvent;

use self::proxy::TCPProxy;
use self::timer::ResetSignal;

mod child;
mod proxy;
mod timer;


#[derive(Clone, Debug, Parser)]
struct Command {
    #[arg(long)]
    dest: SocketAddr,
    #[arg(long)]
    bind: SocketAddr,

    #[arg(long)]
    command: String,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    env_logger::builder().init();

    let cmd = Command::parse();

    info!("Wait for connection...");

    let (network_sender, mut network_receiver) = watch::channel(proxy::TCPEvent::Nothing);
    let p = TCPProxy::new(cmd.dest, cmd.bind, network_sender);

    let (timer_guard, mut handle) = ResetSignal::default().run_after(Duration::from_secs(10));

    tokio::spawn(async move {
        let mut c = child::spawn_child(&cmd.command)?;
        loop {
            select! {
                _ = &mut handle => {
                    info!("Time for app expired");
                    match c.kill_process_group(nix::sys::signal::SIGKILL) {
                        Ok(()) => {
                            info!("Child killed");
                            match c.wait() {
                                Ok(status) => {
                                    info!("Child exited with status {}", status);
                                },
                                Err(e) => {
                                    error!("failed to wait on child: {}", e);
                                },
                            };
                        },
                        Err(e) => {
                            error!("Problem killing child: {}", e);
                        },
                    };
                    break;
                }
                _ = network_receiver.changed() => {
                    let v = network_receiver.borrow();
                    match *v {
                        TCPEvent::DestinationNotResponding => {
                            info!("No destination responded, spawning child");
                            c = child::spawn_child(&cmd.command)?;
                        },
                        TCPEvent::UnknownError => {
                            error!("Some unknown error occured");
                            return Err(anyhow!("Some unknown error occured")) as anyhow::Result<()>;
                        },
                        TCPEvent::GotPacket => {
                            info!("Got packet, restarting cooldown");
                            timer_guard.reset();
                        },
                        TCPEvent::Nothing => {
                            info!("Nothing, ignore me");
                        },
                    };
                }
            }
        }
        Ok(())
    });

    info!("Proxy starting...");

    p.start().await?;

    info!("Proxy exiting...");

    Ok(())
}
