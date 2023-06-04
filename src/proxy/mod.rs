pub mod udp;
pub mod tcp;

#[derive(Debug)]
pub enum ProxyEvent {
    DestinationNotResponding,
    UnknownError,
    GotPacket,
    Nothing,
}
