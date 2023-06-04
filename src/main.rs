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
/// Server Knocker runs a child application and acts like a proxy to it.
///
/// It expects to run a child application and proxy all packets to the port that this child is listening on.
///
/// After a time without receiving any packets (idle_timeout), the child is terminated.
struct Command {
    #[arg(long)]
    /// Address for the proxy to listen on
    listen: SocketAddr,
    #[arg(long)]
    /// Destination address for the proxy
    destination: SocketAddr,

    #[arg(long, default_value_t = String::from("1h"))]
    /// Time between received packets to consider the child applicaiton idle
    ///
    /// Use `h` for hour, `m` for minute, `s` for second or any combination
    idle_timeout: String,
    #[arg(long, default_value_t = String::from("10s"))]
    /// Time to wait between trying to terminate the idle application (SIGTERM) and killing it
    /// (SIGKILL)
    ///
    /// Use `h` for hour, `m` for minute, `s` for second or any combination
    grace_period: String,

    #[arg(long, default_value_t = false)]
    /// (experimental) For TCP proxies: if set the proxy will hold off the request until the child is ready if it
    /// was down.
    ///
    /// This is experimental because it doesn't work properly with child applications that take a
    /// long time to become ready. It will probably need to be paired with a "readiness probe" of sorts in the future.
    hold_packets: bool,

    #[arg(short = 'u', long, default_value_t = false)]
    /// Whether to use UDP instead of the default TCP for the proxy
    udp: bool,

    #[arg(long)]
    /// Command to run as a child. It's expected that it listens on the port set by `dest` and can
    /// be terminated
    ///
    /// Make sure that terminating this command also terminates the child application. If using
    /// Server Knocker with Docker, for example, make sure to not set `-d`.
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
            // cleanup to ensure pending threads are aborted
            handle.abort();
        }
    });

    info!("Proxy starting...");

    if cmd.udp {
        UDPProxy::new(cmd.destination, cmd.listen, network_sender)
            .start()
            .await?;
    } else {
        TCPProxy::new(cmd.destination, cmd.listen, network_sender)
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
