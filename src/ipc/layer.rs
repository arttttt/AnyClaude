use tokio::sync::mpsc;

use super::client::IpcClient;
use super::server::IpcServer;

const IPC_BUFFER: usize = 16;

pub struct IpcLayer;

impl IpcLayer {
    pub fn create() -> (IpcClient, IpcServer) {
        let (sender, receiver) = mpsc::channel(IPC_BUFFER);
        (IpcClient::new(sender), IpcServer::new(receiver))
    }
}
