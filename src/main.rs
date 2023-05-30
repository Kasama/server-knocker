use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use clap::Parser;
use log::{error, info};
use nix::sys::signal::Signal::{SIGKILL, SIGTERM};
use tokio::select;
use tokio::sync::{watch, Notify};

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

    #[arg(long, default_value_t = 10)]
    grace_period: u64,
    #[arg(long, default_value_t = 10)]
    idle_timeout: u64,

    #[arg(long, default_value_t = false)]
    hold_packets: bool,

    #[arg(long)]
    command: String,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    env_logger::builder().init();

    let cmd = Command::parse();

    info!("Wait for connection...");

    let (network_sender, mut network_receiver) = watch::channel(proxy::TCPEvent::Nothing);
    let proxy = TCPProxy::new(cmd.dest, cmd.bind, network_sender);

    let can_proxy_resume = Arc::new(Notify::new());

    let proxy_resume_on_child_creation = can_proxy_resume.clone();
    tokio::spawn(async move {
        let mut children: Vec<_> = vec![child::spawn_child(&cmd.command)?];
        loop {
            let (timer_guard, mut handle) =
                ResetSignal::default().run_after(Duration::from_secs(cmd.idle_timeout));
            select! {
                _ = &mut handle => {
                    children.drain(0..).for_each(|mut c| {
                        info!("Time for app expired. Terminating {} in session {:?}", c.id(), c.get_session_id());
                        match c.kill_process_group(SIGTERM) {
                            Ok(()) => {
                                let _ = c.try_kill_process_group_after(Duration::from_secs(cmd.grace_period), SIGKILL);
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
                    });
                }
                _ = network_receiver.changed() => {
                    let v = network_receiver.borrow();
                    match *v {
                        TCPEvent::DestinationNotResponding => {
                            let c = child::spawn_child(&cmd.command)?;
                            info!("No destination responded, spawning child with id {} in session {:?}", c.id(), c.get_session_id());
                            children.push(c);
                            proxy_resume_on_child_creation.notify_one();
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
    });

    info!("Proxy starting...");

    let r = if cmd.hold_packets {
        Some(can_proxy_resume)
    } else {
        None
    };

    proxy.start(r).await?;

    info!("Proxy exiting...");

    Ok(())
}
