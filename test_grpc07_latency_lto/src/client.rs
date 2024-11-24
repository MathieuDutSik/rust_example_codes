use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use tonic::transport::Endpoint;
use core::time::Duration;
use std::time::Instant;

mod hello_world {
    tonic::include_proto!("helloworld");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let n = 1000;
    let endpoint = "https://localhost:50052";
    //
    {
        println!("---- Cloning the already connected channel ----");
        let endpoint = Endpoint::from_static(endpoint);

        let channel = endpoint.connect()
            .await
            .expect("to obtain a channel");
        let mut total = Duration::default();
        for _i in 0..n {
            let request = tonic::Request::new(HelloRequest {
                name: "World".into(),
            });

            let start = Instant::now(); // Start the time
            let mut client = GreeterClient::new(channel.clone());
            let _response = client.say_hello(request).await?;
            let duration = start.elapsed(); // Measure the elapsed time

            total += duration;
        }
        println!("total={:?}", total);
    }
    //
    {
        println!("---- Cloning the connect_lazy channel ----");
        let endpoint = Endpoint::from_static(endpoint);

        let channel = endpoint.connect_lazy();

        let mut total = Duration::default();
        for _i in 0..n {
            let request = tonic::Request::new(HelloRequest {
                name: "World".into(),
            });

            let start = Instant::now(); // Start the time
            let mut client = GreeterClient::new(channel.clone());
            let _response = client.say_hello(request).await?;
            let duration = start.elapsed(); // Measure the elapsed time

            total += duration;
        }
        println!("total={:?}", total);
    }
    //
    {
        println!("---- Creating the endpoint/channel and connect_lazy----");
        let mut total = Duration::default();
        for _i in 0..n {
            let endpoint = Endpoint::from_static(endpoint);
            let channel = endpoint.connect_lazy();

            let request = tonic::Request::new(HelloRequest {
                name: "World".into(),
            });

            let start = Instant::now(); // Start the time
            let mut client = GreeterClient::new(channel);
            let _response = client.say_hello(request).await?;
            let duration = start.elapsed(); // Measure the elapsed time

            total += duration;
        }
        println!("total={:?}", total);
    }
    //
    {
        println!("---- Creating the endpoint/channel and connect----");
        let mut total = Duration::default();
        for _i in 0..n {
            let endpoint = Endpoint::from_static(endpoint);
            let channel = endpoint.connect()
                .await
                .expect("a connected channel");

            let request = tonic::Request::new(HelloRequest {
                name: "World".into(),
            });

            let start = Instant::now(); // Start the time
            let mut client = GreeterClient::new(channel);
            let _response = client.say_hello(request).await?;
            let duration = start.elapsed(); // Measure the elapsed time

            total += duration;
        }
        println!("total={:?}", total);
    }
    //
    {
        println!("---- Creating the Greeter and iterating over it ----");
        let endpoint = Endpoint::from_static(endpoint);

        let channel = endpoint.connect()
            .await
            .expect("to obtain a channel");
        let mut client = GreeterClient::new(channel);
        let mut total = Duration::default();
        for _i in 0..n {
            let request = tonic::Request::new(HelloRequest {
                name: "World".into(),
            });

            let start = Instant::now(); // Start the time
            let _response = client.say_hello(request).await?;
            let duration = start.elapsed(); // Measure the elapsed time

            total += duration;
        }
        println!("total={:?}", total);
    }
    Ok(())
}
