use std::net::Ipv4Addr;
use std::os::unix::net::UnixStream;
use std::io::prelude::*;

use std::io;
use std::os::unix::net::UnixDatagram;
use std::thread;

const SOCKET_PATH_PREFIX: &str = "/tmp/client_";
const BUFFER_SIZE: usize = 256;

pub async fn mobman() -> Result<(), String> {
    let mut poll = mio::Poll::new().unwrap();
    let mut events = mio::Events::with_capacity(1024);
    let mut buf = [0; 65535];

    let bind_addr = "0.0.0.0:1900";
    let mc_addr = Ipv4Addr::new(224, 0, 0, 0);
    let if_addr = Ipv4Addr::UNSPECIFIED;
    let mut socket =
        mio::net::UdpSocket::bind(bind_addr.parse().unwrap()).unwrap();

    socket.join_multicast_v4(&mc_addr, &if_addr);

    poll.registry()
        .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
        .unwrap();

    loop {
        poll.poll(&mut events, None).unwrap();

        for event in &events {
            if event.token() == mio::Token(0) {
                let number_bytes = socket.recv(&mut buf).expect("failure!");
                println!("num bytes: {}", number_bytes);
                let filled_buf = &mut buf[..number_bytes];
                let added_ip = String::from_utf8(filled_buf.to_vec()).unwrap();
            
                println!("{}", added_ip);
            }
        }
    }

    Ok(())
}

pub async fn mobman_unix() -> std::io::Result<()> {

    // Generate a unique socket path for the client
    let unique_socket_path = format!("{}{}", SOCKET_PATH_PREFIX, std::process::id());

    // Create a Unix domain datagram socket
    let client_socket = UnixDatagram::bind(&unique_socket_path)?;

    // Set up server address
    let server_address = "/tmp/netmon_pubsub_server";

    // Subscribe to the server
    client_socket.send_to(b"subscribe", server_address)?;

    let mut buffer = [0; BUFFER_SIZE];
    loop {
        match client_socket.recv_from(&mut buffer) {
            Ok((bytes_received, _)) => {
                let message = String::from_utf8_lossy(&buffer[..bytes_received]);
                println!("Received message: {}", message);
            }
            Err(err) => {
                eprintln!("Error receiving message: {}", err);
                break;
            }
        }
    }

    println!("You probably didn't start the netmon server");
    Ok(())    
}