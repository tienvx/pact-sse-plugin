use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;

use env_logger::Env;
use tokio::net::TcpListener;
use tonic::transport::Server;
use uuid::Uuid;

use pact_sse_plugin::{SsePactPlugin, TcpIncoming};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env = Env::new().filter("LOG_LEVEL");
    env_logger::init_from_env(env);

    let addr: SocketAddr = "0.0.0.0:0".parse()?;
    let listener = TcpListener::bind(addr).await?;
    let address = listener.local_addr()?;

    let server_key = Uuid::new_v4().to_string();
    println!(
        r#"{{"port":{}, "serverKey":"{}"}}"#,
        address.port(),
        server_key
    );
    let _ = std::io::stdout().flush();

    let plugin = SsePactPlugin {
        mock_servers: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
    };

    Server::builder()
        .add_service(pact_sse_plugin::proto::pact_plugin_server::PactPluginServer::new(plugin))
        .serve_with_incoming(TcpIncoming { inner: listener })
        .await?;

    Ok(())
}
