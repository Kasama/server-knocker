use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::anyhow;
use log::{error, info};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{channel, Receiver};
use tokio::sync::watch::Sender;

use super::ProxyEvent;

pub struct UDPProxy {
    destination: SocketAddr,
    listen_addr: SocketAddr,
    notification: Arc<Sender<ProxyEvent>>,
}

// max size of an UDP packet is 65507 bytes for IPv4 and 65527 bytes for IPv6,
// so 64 * 1024 is a nice "round" number that will be enough for any UDP packet.
// UDP doesn't have fragmentation, so using a smaller buffer would simply drop content
// that's larger than the buffer size.
const UDP_MAX_PACKET_SIZE: usize = 64 * 1024;

impl UDPProxy {
    pub fn new(
        destination: SocketAddr,
        listen_addr: SocketAddr,
        notification: Sender<ProxyEvent>,
    ) -> Self {
        Self {
            destination,
            listen_addr,
            notification: Arc::new(notification),
        }
    }
    async fn respond(
        socket: Arc<UdpSocket>,
        mut receiver: Receiver<(SocketAddr, Vec<u8>)>,
    ) -> anyhow::Result<()> {
        loop {
            let (addr, buf) = receiver
                .recv()
                .await
                .ok_or(anyhow!("sender channel closed"))?;
            let to_forward = buf.as_slice();

            socket.send_to(to_forward, addr).await?;

            if false {
                break;
            }
        }
        Ok(()) as anyhow::Result<()>
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        let local = Arc::new(UdpSocket::bind(self.listen_addr).await?);

        let (response_sender, receiver) = channel::<(SocketAddr, Vec<u8>)>(512);
        let response_socket = local.clone();
        tokio::spawn(async move {
            match Self::respond(response_socket, receiver).await {
                Ok(_) => {}
                Err(e) => {
                    error!("error responding: {e}");
                }
            }
        });

        let mut client_map = HashMap::new();

        let mut buf = [0; UDP_MAX_PACKET_SIZE];
        loop {
            let lo = local.clone();
            let (read_bytes, src_addr) = lo.recv_from(&mut buf).await?;

            let client_id = format!("{}", src_addr);

            let _ = self.notification.send(ProxyEvent::GotPacket);
            if src_addr == self.destination {
                info!(
                    "ignoring packet from destination: {src_addr} in {}",
                    lo.local_addr()?.port()
                );
                continue;
            } else {
                info!(
                    "got packet from {}: {:?}",
                    src_addr,
                    buf[..read_bytes].to_vec()
                );
            }

            let i_response_sender = response_sender.clone();
            let sender = client_map.entry(client_id.clone()).or_insert_with(|| {
                let (client_sender, mut client_receiver) = channel::<Vec<u8>>(512);

                let mut destination_listener_addr = self.listen_addr;
                destination_listener_addr.set_port(0);
                let destination_addr = self.destination;
                let b = async move {
                    let backend_listener =
                        Arc::new(UdpSocket::bind(destination_listener_addr).await?);

                    backend_listener.connect(destination_addr).await?;

                    info!(
                        "New client registered. Socket opened on {:?}",
                        backend_listener.local_addr()
                    );

                    let backend_sender = backend_listener.clone();
                    tokio::spawn(async move {
                        loop {
                            let buf = client_receiver
                                .recv()
                                .await
                                .ok_or(anyhow!("backend client channel closed"))?;

                            backend_sender.send_to(&buf[..], destination_addr).await?;

                            if false {
                                break;
                            }
                        }

                        Ok(()) as anyhow::Result<()>
                    });

                    let mut buf = [0; UDP_MAX_PACKET_SIZE];
                    loop {
                        let read_bytes = backend_listener.recv(&mut buf).await?;

                        i_response_sender
                            .send((src_addr, buf[..read_bytes].to_vec()))
                            .await?;

                        if false {
                            break;
                        }
                    }

                    Ok(()) as anyhow::Result<()>
                };
                tokio::spawn(async move {
                    match b.await {
                        Ok(_) => {}
                        Err(e) => {
                            error!("error on thing: {e}");
                        }
                    }
                });

                client_sender
            });

            match sender.send(buf[..read_bytes].to_vec()).await {
                Ok(_) => {}
                Err(e) => {
                    error!("Could not send to {client_id}: {e}");
                    client_map.remove(&client_id.clone());
                    let _ = self.notification.send(ProxyEvent::DestinationNotResponding);
                }
            };
        }
    }
}
