use tonic::transport::{ClientTlsConfig, Certificate};
use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use tonic::transport::{Channel, Endpoint};

mod hello_world {
    tonic::include_proto!("helloworld");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pem = std::fs::read_to_string("self_signed_cert.pem")?;
    let ca_certificate = Certificate::from_pem(pem);

    let tls_config = ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_certificate);

    let mut client = if false {
        // That does not work
        let endpoint = Endpoint::from_shared("https://localhost:50051")?
            .tls_config(tls_config)?;
        GreeterClient::connect(endpoint).await?
    } else {
        // That works
        let channel = Channel::from_static("https://localhost:50051")
            .tls_config(tls_config)?
            .connect()
            .await?;
        GreeterClient::new(channel)
    };

    let request = tonic::Request::new(HelloRequest {
        name: "World".into(),
    });

    let response = client.say_hello(request).await?;
    println!("Response from server: {:?}", response);

    Ok(())
}
