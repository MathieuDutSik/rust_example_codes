use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use tonic::transport::{Channel, Endpoint};
use core::time::Duration;

mod hello_world {
    tonic::include_proto!("helloworld");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connect_timeout = Duration::from_millis(10);
    let timeout = Duration::from_millis(10);

    let endpoint = Endpoint::from_static("https://localhost:50052");
    let endpoint = endpoint.connect_timeout(connect_timeout);
    let endpoint = endpoint.timeout(timeout);


    let channel = endpoint.connect()
        .await
        .expect("to obtain a channel");

    let channel_copy : Channel = channel.clone();
//    println!("Before client creation i={}", i);
    let mut client = GreeterClient::new(channel_copy);
//    println!("After client creation i={}", i);


    for i in 0..100 {
        println!("Before sleep i={}", i);
        tokio::time::sleep(Duration::from_millis(1000)).await;
        println!(" After sleep i={}", i);

        let request = tonic::Request::new(HelloRequest {
            name: "World".into(),
        });

        let response = client.say_hello(request).await?;
        println!("Response from server i={} response={:?}", i, response);
    }
    Ok(())
}
