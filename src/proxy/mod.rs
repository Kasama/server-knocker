use std::net::SocketAddr;
use std::sync::Arc;

use log::info;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, UdpSocket};
use tokio::sync::watch::Sender;
use tokio::sync::Notify;

pub mod tcp;

pub struct TCPProxy {
    destination: SocketAddr,
    listen_addr: SocketAddr,
    notification: Arc<Sender<TCPEvent>>,
}

#[derive(Debug)]
pub enum TCPEvent {
    DestinationNotResponding,
    UnknownError,
    GotPacket,
    Nothing,
}

impl TCPProxy {
    pub fn new(
        destination: SocketAddr,
        listen_addr: SocketAddr,
        notification: Sender<TCPEvent>,
    ) -> Self {
        Self {
            destination,
            listen_addr,
            notification: Arc::new(notification),
        }
    }

    async fn pipe_sockets<R, W>(
        mut reader: R,
        mut writer: W,
        notification: Arc<Sender<TCPEvent>>,
    ) -> anyhow::Result<()>
    where
        R: AsyncReadExt + Unpin,
        W: AsyncWriteExt + Unpin,
    {
        // default MTU in most places is 1500 bytes.
        // So I think it's ok to have a buffer of this size.
        // The only difference it makes is that it will split the packets sent to the destination
        // into packets of at most this size.
        const BUFFER_SIZE: usize = 1536;
        let mut reader_buffer = [0; BUFFER_SIZE];
        loop {
            let bytes_read = reader.read(&mut reader_buffer).await?;
            if bytes_read == 0 {
                break;
            }
            writer.write_all(&reader_buffer[..bytes_read]).await?;
            let _ = notification.send_replace(TCPEvent::GotPacket);
        }

        Ok(())
    }

    // pub async fn start_udp(&self, can_resume: Option<Arc<Notify>>) -> anyhow::Result<()> {
    //     let input_socket = UdpSocket::bind(self.listen_addr).await?;

    //     'accept_connection: loop {
    //         info!("receiving a new connection");

    //         input_socket.connect(self.destination).await?;

    //         let output_socket = loop {
    //             match if self.destination.is_ipv4() {
    //                 UdpSocket::new_v4()
    //             } else {
    //                 UdpSocket::new_v6()
    //             }?
    //             .connect(self.destination)
    //             .await
    //             {
    //                 Ok(s) => break Ok(s),
    //                 Err(e) => {
    //                     match e.kind() {
    //                         std::io::ErrorKind::BrokenPipe
    //                         | std::io::ErrorKind::ConnectionAborted
    //                         | std::io::ErrorKind::TimedOut
    //                         | std::io::ErrorKind::ConnectionReset
    //                         | std::io::ErrorKind::ConnectionRefused => {
    //                             let _ = self
    //                                 .notification
    //                                 .send_replace(TCPEvent::DestinationNotResponding);
    //                             info!("Waiting to be able to resume");
    //                             if let Some(ref r) = can_resume {
    //                                 r.notified().await;
    //                             } else {
    //                                 continue 'accept_connection;
    //                             }
    //                             info!("Resuming...");
    //                         }
    //                         _ => {
    //                             info!("Something unexpected happened to the destination: {:?}", e);
    //                             let _ = self.notification.send_replace(TCPEvent::UnknownError);
    //                             break (Err(e));
    //                         }
    //                     };
    //                     continue;
    //                 }
    //             };
    //         }?;

    //         let (input_socket_reader, input_socket_writer) = input_socket.into_split();
    //         let (output_socket_reader, output_socket_writer) = output_socket.into_split();

    //         tokio::task::spawn(Self::pipe_sockets(
    //             input_socket_reader,
    //             output_socket_writer,
    //             self.notification.clone(),
    //         ));
    //         tokio::task::spawn(Self::pipe_sockets(
    //             output_socket_reader,
    //             input_socket_writer,
    //             self.notification.clone(),
    //         ));
    //     }
    // }

    pub async fn start(&self, can_resume: Option<Arc<Notify>>) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.listen_addr).await?;

        'accept_connection: loop {
            let (input_socket, _) = listener.accept().await?;
            info!("receiving a new connection");

            let output_socket = loop {
                match if self.destination.is_ipv4() {
                    TcpSocket::new_v4()
                } else {
                    TcpSocket::new_v6()
                }?
                .connect(self.destination)
                .await
                {
                    Ok(s) => break Ok(s),
                    Err(e) => {
                        match e.kind() {
                            std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionRefused => {
                                let _ = self
                                    .notification
                                    .send_replace(TCPEvent::DestinationNotResponding);
                                info!("Waiting to be able to resume");
                                if let Some(ref r) = can_resume {
                                    r.notified().await;
                                } else {
                                    continue 'accept_connection;
                                }
                                info!("Resuming...");
                            }
                            _ => {
                                info!("Something unexpected happened to the destination: {:?}", e);
                                let _ = self.notification.send_replace(TCPEvent::UnknownError);
                                break (Err(e));
                            }
                        };
                        continue;
                    }
                };
            }?;

            let (input_socket_reader, input_socket_writer) = input_socket.into_split();
            let (output_socket_reader, output_socket_writer) = output_socket.into_split();

            tokio::task::spawn(Self::pipe_sockets(
                input_socket_reader,
                output_socket_writer,
                self.notification.clone(),
            ));
            tokio::task::spawn(Self::pipe_sockets(
                output_socket_reader,
                input_socket_writer,
                self.notification.clone(),
            ));
        }
    }
}

#[cfg(test)]
mod test {
    use anyhow::Result;
    use tokio::net::{TcpListener, UdpSocket};

    #[tokio::test]
    async fn tcp() {
        let listener = TcpListener::bind("::1:8080").await;

        match listener {
            Ok(_) => (),
            Err(e) => panic!("Failed with error: {}", e),
        }
    }

    #[tokio::test]
    async fn udp() -> Result<()>{
        let sock = UdpSocket::bind("0.0.0.0:8002").await?;
        let mut buf = [0; 4096];

        loop {
            let (bytes_read, _addr) = sock.recv_from(&mut buf).await?;
            if bytes_read == 0 {
                break;
            }

            sock.send_to(&buf, "127.0.0.1:8001").await?;
        }

        Ok(())
    }
}
