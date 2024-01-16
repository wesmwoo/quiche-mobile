use mio::unix::pipe;
use std::thread;
use std::net::SocketAddr;

use std::io::{self, Read, Write};
use std::os::unix::net::UnixDatagram;

const SOCKET_PATH_PREFIX: &str = "/tmp/client_";
const BUFFER_SIZE: usize = 256;

pub struct MobileConnection {
    handle: thread::JoinHandle<i8>,
    receiver: mio::unix::pipe::Receiver,
}

impl MobileConnection {
    pub fn new() -> Self {
        let (mut sender, receiver) = pipe::new().unwrap();
        let handle = thread::spawn(move || {
            // Generate a unique socket path for the client
            let unique_socket_path = format!("{}{}", SOCKET_PATH_PREFIX, std::process::id());

            // Create a Unix domain datagram socket
            let client_socket = UnixDatagram::bind(&unique_socket_path).unwrap();

            // Set up server address
            let server_address = "/tmp/netmon_pubsub_server";

            // Subscribe to the server
            client_socket.send_to(b"subscribe", server_address).unwrap();
            
            const PING: &[u8; 1] = b"p";
            const START: &[u8; 1] = b"s";
            const END: &[u8; 1] = b"e";



            let mut buffer = [0; BUFFER_SIZE];
            loop {
                match client_socket.recv_from(&mut buffer) {
                    Ok((bytes_received, _)) => {
                        let message = String::from_utf8_lossy(&buffer[..bytes_received]);
                        println!("Received message: {}", message);
                        println!("num bytes: {}", bytes_received);
                        // println!("message: {}\n== START: {}\n== END: {}",
                            // message, message == "START", message == "END");

                        if message == "START" {
                            sender.write(START);
                        } else if message == "END" {
                            sender.write(END);
                        } else {
                            sender.write(PING);
                        }
                    }
                    
                    Err(err) => {
                        eprintln!("Error receiving message: {}", err);
                        break;
                    }
                }
            }
            // let mut poll = mio::Poll::new().unwrap();
            // let mut events = mio::Events::with_capacity(1024);
            // let mut buf = [0; 65535];
            // let bind_addr = "0.0.0.0:1900";
            // let mc_addr = Ipv4Addr::new(224, 0, 0, 0);
            // let if_addr = Ipv4Addr::UNSPECIFIED;
            // let mut socket =
            //     mio::net::UdpSocket::bind(bind_addr.parse().unwrap()).unwrap();

            // const PING: &[u8; 1] = b"m";

            // socket.join_multicast_v4(&mc_addr, &if_addr);

            // poll.registry()
            //     .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
            //     .unwrap();

            // loop {
            //     poll.poll(&mut events, None).unwrap();
        
            //     for event in &events {
            //         if event.token() == mio::Token(0) {
            //             let number_bytes = socket.recv(&mut buf).expect("failure!");
            //             println!("num bytes: {}", number_bytes);
            //             let filled_buf = &mut buf[..number_bytes];
            //             let added_ip = String::from_utf8(filled_buf.to_vec()).unwrap();
                    
            //             println!("{}", added_ip);
            //             sender.write(PING);
                        
            //         }
            //     }
            // }
            return -1
        });
        
        MobileConnection {
            handle: handle,
            receiver: receiver,
        }
    }

    pub fn get_receiver(self) -> mio::unix::pipe::Receiver {
        return self.receiver
    }

    pub fn handle_detected_migration(&mut self, conn: &mut quiche::Connection, local_addr: SocketAddr, peer_addr: SocketAddr) {
        conn.probe_path(local_addr, peer_addr);
    }

}