use std::net::Ipv4Addr;

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