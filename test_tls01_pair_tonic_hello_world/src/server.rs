use tonic::{transport::Server, transport::ServerTlsConfig, Request, Response, Status};
use tonic::transport::Identity;

mod hello_world {
    tonic::include_proto!("helloworld");
}

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};

#[derive(Debug, Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(&self, request: Request<HelloRequest>) -> Result<Response<HelloReply>, Status> {
        println!("Passing by say_hello");
        let reply = hello_world::HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
//    let addr = "[::1]:50051".parse()?;

    let cert = std::fs::read_to_string("self_signed_cert.pem")?;
    let key = std::fs::read_to_string("private_key.pem")?;
    let identity = Identity::from_pem(cert, key);
    println!("identity={:?}", identity);

    let tls_config = ServerTlsConfig::new()
        .identity(identity);

    let addr = "127.0.0.1:50051".parse()?;
//    let addr = "localhost:50051".parse()?;

    let greeter = MyGreeter::default();

    Server::builder()
        .tls_config(tls_config)?
        .add_service(GreeterServer::new(greeter))
        .serve(addr)
        .await?;

    Ok(())
}
