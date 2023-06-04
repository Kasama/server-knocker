use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::anyhow;
use clap::Parser;
use log::{debug, error, info, trace};
use nix::sys::signal::Signal::{SIGKILL, SIGTERM};
use tokio::select;
use tokio::sync::{watch, Notify};

use crate::child::LinuxChild;
use crate::proxy::ProxyEvent;

use self::proxy::tcp::TCPProxy;
use self::proxy::udp::UDPProxy;
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

    #[arg(long, default_value_t = String::from("10s"))]
    grace_period: String,
    #[arg(long, default_value_t = String::from("10s"))]
    idle_timeout: String,

    #[arg(long, default_value_t = false)]
    hold_packets: bool,

    #[arg(short = 'u', long, default_value_t = false)]
    udp: bool,

    #[arg(long)]
    command: String,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    env_logger::builder().init();

    let cmd = Command::parse();

    let idle_timeout = parse_duration::parse(&cmd.idle_timeout)?;
    let grace_period = parse_duration::parse(&cmd.grace_period)?;

    info!("Wait for connection...");

    let (network_sender, mut network_receiver) = watch::channel(proxy::ProxyEvent::Nothing);

    let can_proxy_resume = Arc::new(Notify::new());

    let proxy_resume_on_child_creation = can_proxy_resume.clone();
    let process_handler = tokio::spawn(async move {
        let mut children = vec![child::spawn_child(&cmd.command)?];
        loop {
            let (timer_guard, mut handle) = ResetSignal::default().run_after(idle_timeout);
            select! {
                _ = &mut handle => {
                    children.drain(0..).for_each(|mut c| {
                        debug!("Time for app expired. Terminating {} in session {:?}", c.id(), c.get_session_id());
                        match c.kill_process_group(SIGTERM) {
                            Ok(()) => {
                                let _ = c.try_kill_process_group_after(grace_period, SIGKILL);
                                info!("Child terminated");
                                match c.wait() {
                                    Ok(status) => {
                                        debug!("Child exited with status {}", status);
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
                        ProxyEvent::DestinationNotResponding => {
                            let c = child::spawn_child(&cmd.command)?;
                            info!("No response from destination, spawning command");
                            debug!("Command has id {} in session {:?}", c.id(), c.get_session_id());
                            children.push(c);
                            proxy_resume_on_child_creation.notify_one();
                        },
                        ProxyEvent::UnknownError => {
                            error!("Some unknown error occured");
                            return Err(anyhow!("Some unknown error occured")) as anyhow::Result<()>;
                        },
                        ProxyEvent::GotPacket => {
                            debug!("Got packet, restarting cooldown");
                            timer_guard.reset();
                        },
                        ProxyEvent::Nothing => {
                            trace!("Got a TCPEvent of nothing");
                        },
                    };
                }
            }
        }
    });

    info!("Proxy starting...");

    if cmd.udp {
        UDPProxy::new(cmd.dest, cmd.bind, network_sender)
            .start(None)
            .await?;
    } else {
        TCPProxy::new(cmd.dest, cmd.bind, network_sender)
            .start(if cmd.hold_packets {
                Some(can_proxy_resume)
            } else {
                None
            })
            .await?;
    }

    info!("Proxy exiting...");

    process_handler.await?
}
