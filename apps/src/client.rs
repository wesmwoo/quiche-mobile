// Copyright (C) 2020, Cloudflare, Inc.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
//     * Redistributions of source code must retain the above copyright notice,
//       this list of conditions and the following disclaimer.
//
//     * Redistributions in binary form must reproduce the above copyright
//       notice, this list of conditions and the following disclaimer in the
//       documentation and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
// IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO,
// THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
// PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR
// CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
// EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
// PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
// PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
// LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
// NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use crate::args::*;
use crate::common::*;

use std::net::ToSocketAddrs;

use std::io::prelude::*;

use std::rc::Rc;

use std::cell::RefCell;

use std::{thread, time};
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::io::{self, Read, Write};
use mio::unix::pipe;

use ring::rand::*;

use local_ip_address::local_ip;
use local_ip_address::list_afinet_netifas;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

const MAX_DATAGRAM_SIZE: usize = 1350;

#[derive(Debug)]
pub enum ClientError {
    HandshakeFail,
    HttpFail,
    Other(String),
}

pub fn connect(
    args: ClientArgs, conn_args: CommonArgs,
    output_sink: impl FnMut(String) + 'static,
) -> Result<(), ClientError> {
    let mut buf = [0; 65535];
    let mut out = [0; MAX_DATAGRAM_SIZE];

    println!("local ip address {}", local_ip_address::local_ip().unwrap());

    // Set up state logic for detecting change in IP
    let mut local_ip = local_ip().unwrap();
    let enable_ip_migration = false;
    let enable_enhanced_ip_mig = enable_ip_migration && true;

    let mut network_interfaces = list_afinet_netifas().unwrap();
    for (name, ip) in network_interfaces.iter() {
       if name == "enp0s8" {
           local_ip = *ip;
       }
       println!("[initial] name: {} ip: {}", name, ip);
    }

    // TODO turn this variable into a shared?
    let mut ip_migrate_bool = false;
    let mut test_ip = local_ip;

    // println!("local_ip {} {}", local_ip, local_ip == IpAddr::V4(Ipv4Addr::new(172, 29, 73, 111)));

    let output_sink =
        Rc::new(RefCell::new(output_sink)) as Rc<RefCell<dyn FnMut(_)>>;

    // Setup the event loop.
    let mut poll = mio::Poll::new().unwrap();
    let mut events = mio::Events::with_capacity(1024);

    // We'll only connect to the first server provided in URL list.
    let connect_url = &args.urls[0];

    // Resolve server address.
    let peer_addr = if let Some(addr) = &args.connect_to {
        addr.parse().expect("--connect-to is expected to be a string containing an IPv4 or IPv6 address with a port. E.g. 192.0.2.0:443")
    } else {
        connect_url.to_socket_addrs().unwrap().next().unwrap()
    };

    // Bind to INADDR_ANY or IN6ADDR_ANY depending on the IP family of the
    // server address. This is needed on macOS and BSD variants that don't
    // support binding to IN6ADDR_ANY for both v4 and v6.
    let bind_addr = match peer_addr {
        std::net::SocketAddr::V4(_) => format!("0.0.0.0:{}", args.source_port),
        std::net::SocketAddr::V6(_) => format!("[::]:{}", args.source_port),
    };

    // Create the UDP socket backing the QUIC connection, and register it with
    // the event loop.
    let mut socket =
        mio::net::UdpSocket::bind(bind_addr.parse().unwrap()).unwrap();
    poll.registry()
        .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
        .unwrap();

    let migrate_socket = if args.perform_migration {
        let mut socket =
            mio::net::UdpSocket::bind(bind_addr.parse().unwrap()).unwrap();
        poll.registry()
            .register(&mut socket, mio::Token(1), mio::Interest::READABLE)
            .unwrap();

        Some(socket)
    } else {
        None
    };

    let mut ip_mig_socket = None;

    // Create the configuration for the QUIC connection.
    let mut config = quiche::Config::new(args.version).unwrap();

    config.verify_peer(!args.no_verify);

    config.set_application_protos(&conn_args.alpns).unwrap();

    config.set_max_idle_timeout(conn_args.idle_timeout);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(conn_args.max_data);
    config.set_initial_max_stream_data_bidi_local(conn_args.max_stream_data);
    config.set_initial_max_stream_data_bidi_remote(conn_args.max_stream_data);
    config.set_initial_max_stream_data_uni(conn_args.max_stream_data);
    config.set_initial_max_streams_bidi(conn_args.max_streams_bidi);
    config.set_initial_max_streams_uni(conn_args.max_streams_uni);
    config.set_disable_active_migration(!conn_args.enable_active_migration);
    config.set_active_connection_id_limit(conn_args.max_active_cids);

    config.set_max_connection_window(conn_args.max_window);
    config.set_max_stream_window(conn_args.max_stream_window);

    let mut keylog = None;

    if let Some(keylog_path) = std::env::var_os("SSLKEYLOGFILE") {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(keylog_path)
            .unwrap();

        keylog = Some(file);

        config.log_keys();
    }

    if conn_args.no_grease {
        config.grease(false);
    }

    if conn_args.early_data {
        config.enable_early_data();
    }

    config
        .set_cc_algorithm_name(&conn_args.cc_algorithm)
        .unwrap();

    if conn_args.disable_hystart {
        config.enable_hystart(false);
    }

    if conn_args.dgrams_enabled {
        config.enable_dgram(true, 1000, 1000);
    }

    let mut http_conn: Option<Box<dyn HttpConn>> = None;

    let mut app_proto_selected = false;

    // Generate a random source connection ID for the connection.
    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
    let rng = SystemRandom::new();
    rng.fill(&mut scid[..]).unwrap();

    let scid = quiche::ConnectionId::from_ref(&scid);

    let local_addr = socket.local_addr().unwrap();

    // Create a QUIC connection and initiate handshake.
    let mut conn = quiche::connect(
        connect_url.domain(),
        &scid,
        local_addr,
        peer_addr,
        &mut config,
    )
    .unwrap();

    if let Some(keylog) = &mut keylog {
        if let Ok(keylog) = keylog.try_clone() {
            conn.set_keylog(Box::new(keylog));
        }
    }

    // Only bother with qlog if the user specified it.
    #[cfg(feature = "qlog")]
    {
        if let Some(dir) = std::env::var_os("QLOGDIR") {
            let id = format!("{scid:?}");
            let writer = make_qlog_writer(&dir, "client", &id);

            conn.set_qlog(
                std::boxed::Box::new(writer),
                "quiche-client qlog".to_string(),
                format!("{} id={}", "quiche-client qlog", id),
            );
        }
    }

    if let Some(session_file) = &args.session_file {
        if let Ok(session) = std::fs::read(session_file) {
            conn.set_session(&session).ok();
        }
    }

    info!(
        "connecting to {:} from {:} with scid {:?}",
        peer_addr,
        socket.local_addr().unwrap(),
        scid,
    );

    let (write, send_info) = conn.send(&mut out).expect("initial send failed");

    while let Err(e) = socket.send_to(&out[..write], send_info.to) {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            trace!(
                "{} -> {}: send() would block",
                socket.local_addr().unwrap(),
                send_info.to
            );
            continue;
        }

        return Err(ClientError::Other(format!("send() failed: {e:?}")));
    }

    trace!("written {}", write);

    let app_data_start = std::time::Instant::now();

    let mut pkt_count = 0;

    let mut scid_sent = false;
    let mut new_path_probed = false;
    let mut migrated = false;

    let mut i = 0;

    // Set up signalling for managing concurrent thread
    let stop = Arc::new(AtomicBool::new(false));
    let stop_me = stop.clone();
    const PIPE_RECV: mio::Token = mio::Token(4);

    let (mut sender, mut receiver) = pipe::new().unwrap();

    poll.registry().register(&mut receiver, PIPE_RECV, mio::Interest::READABLE).unwrap(); 

    if enable_enhanced_ip_mig {
        // Set up the thread for detecting changes in IP address
        let handle = thread::spawn(move || {
            let mut i = 0;
            
            let mut original_ip = local_ip_address::local_ip().unwrap();

            let mut network_interfaces = list_afinet_netifas().unwrap();
            for (name, ip) in network_interfaces.iter() {
                if name == "enp0s8" {
                original_ip = *ip;
                }
                info!("[thread] [initial] name: {} ip: {}", name, ip);
            }

            let mut ip_migrate_bool = false;
            let mut test_ip = original_ip;

            
            loop {
                thread::sleep(Duration::from_millis(5));
                i += 1;
                network_interfaces = list_afinet_netifas().unwrap();
                for (name, ip) in network_interfaces.iter() {
                    if name == "enp0s8" {
                        test_ip = *ip; // TODO this logic fails with multiple IPs...
                    }
                    println!("[thread] [check] name: {} ip: {}", name, ip);
                    info!("[thread] [check] name: {} ip: {}", name, ip);
                }
            
                // Below is for an incomplete implementation of "simple" IP migration support -- TODO
                // consume a connection ID
                println!("[thread] test_ip: {} original_ip: {}, are_equal is {}", test_ip, original_ip, test_ip == original_ip);
                // info!("[thread] test_ip: {} original_ip: {}, are_equal is {}", test_ip, original_ip, test_ip == original_ip);
                if test_ip != original_ip {
                    println!("[thread] test_ip not equal to local! {:?} {:?} ", test_ip, original_ip);
                    info!("[thread] test_ip not equal to local! {:?} {:?} ", test_ip, original_ip);
                    original_ip = test_ip;
                    info!("[thread] test_ip is now: {:?}", test_ip);
                    println!("[thread] test_ip is now: {:?}", test_ip);
                    // TODO: fire an event that is registered with the poll?
                    drop(sender);
                    println!("[thread] dropped sender");
                    break;
                }
                info!("[thread] iterations = {}", i);
                if stop_me.load(Ordering::Relaxed) {
                    info!("iterations = {}", i);
                    break;
                }
            }
        });
    }

    loop {
        if !conn.is_in_early_data() || app_proto_selected {
            poll.poll(&mut events, conn.timeout()).unwrap();
        }


        // If the event loop reported no events, it means that the timeout
        // has expired, so handle it without attempting to read packets. We
        // will then proceed with the send loop.
        if events.is_empty() {
            trace!("timed out");
            conn.on_timeout();
        }

        // Read incoming UDP packets from the socket and feed them to quiche,
        // until there are no more packets to read.
        for event in &events {

            if event.is_read_closed() {
                info!("sender dropped");
                println!("sender dropped");
                continue;
            }

            let socket = match event.token() {
                mio::Token(0) => &socket,

                mio::Token(1) => migrate_socket.as_ref().unwrap(),
                    
                mio::Token(2) => ip_mig_socket.as_ref().unwrap(),

                _ => unreachable!(),
            };

            let local_addr = socket.local_addr().unwrap();
            'read: loop {
                let (len, from) = match socket.recv_from(&mut buf) {
                    Ok(v) => v,

                    Err(e) => {
                        // There are no more UDP packets to read on this socket.
                        // Process subsequent events.
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            trace!("{}: recv() would block", local_addr);
                            break 'read;
                        }

                        return Err(ClientError::Other(format!(
                            "{local_addr}: recv() failed: {e:?}"
                        )));
                    },
                };

                trace!("{}: got {} bytes", local_addr, len);

                if let Some(target_path) = conn_args.dump_packet_path.as_ref() {
                    let path = format!("{target_path}/{pkt_count}.pkt");

                    if let Ok(f) = std::fs::File::create(path) {
                        let mut f = std::io::BufWriter::new(f);
                        f.write_all(&buf[..len]).ok();
                    }
                }

                pkt_count += 1;

                let recv_info = quiche::RecvInfo {
                    to: local_addr,
                    from,
                };

                // Process potentially coalesced packets.
                let read = match conn.recv(&mut buf[..len], recv_info) {
                    Ok(v) => v,

                    Err(e) => {
                        error!("{}: recv failed: {:?}", local_addr, e);
                        continue 'read;
                    },
                };

                trace!("{}: processed {} bytes", local_addr, read);
            }
        }

        trace!("done reading");

        if conn.is_closed() {
            // Join the IP change detection thread
            // handle.join().unwrap();
            stop.store(true, Ordering::Relaxed);
            info!("Stop set to true -- thread should close");

            info!(
                "connection closed, {:?} {:?}",
                conn.stats(),
                conn.path_stats().collect::<Vec<quiche::PathStats>>()
            );

            if !conn.is_established() {
                error!(
                    "connection timed out after {:?}",
                    app_data_start.elapsed(),
                );

                return Err(ClientError::HandshakeFail);
            }

            if let Some(session_file) = &args.session_file {
                if let Some(session) = conn.session() {
                    std::fs::write(session_file, session).ok();
                }
            }

            if let Some(h_conn) = http_conn {
                if h_conn.report_incomplete(&app_data_start) {
                    return Err(ClientError::HttpFail);
                }
            }

            break;
        }

        // Create a new application protocol session once the QUIC connection is
        // established.
        if (conn.is_established() || conn.is_in_early_data()) &&
            (!args.perform_migration || migrated) &&
            !app_proto_selected
        {
            // At this stage the ALPN negotiation succeeded and selected a
            // single application protocol name. We'll use this to construct
            // the correct type of HttpConn but `application_proto()`
            // returns a slice, so we have to convert it to a str in order
            // to compare to our lists of protocols. We `unwrap()` because
            // we need the value and if something fails at this stage, there
            // is not much anyone can do to recover.

            let app_proto = conn.application_proto();

            if alpns::HTTP_09.contains(&app_proto) {
                http_conn = Some(Http09Conn::with_urls(
                    &args.urls,
                    args.reqs_cardinal,
                    Rc::clone(&output_sink),
                ));

                app_proto_selected = true;
            } else if alpns::HTTP_3.contains(&app_proto) {
                let dgram_sender = if conn_args.dgrams_enabled {
                    Some(Http3DgramSender::new(
                        conn_args.dgram_count,
                        conn_args.dgram_data.clone(),
                        0,
                    ))
                } else {
                    None
                };

                http_conn = Some(Http3Conn::with_urls(
                    &mut conn,
                    &args.urls,
                    args.reqs_cardinal,
                    &args.req_headers,
                    &args.body,
                    &args.method,
                    args.send_priority_update,
                    conn_args.max_field_section_size,
                    conn_args.qpack_max_table_capacity,
                    conn_args.qpack_blocked_streams,
                    args.dump_json,
                    dgram_sender,
                    Rc::clone(&output_sink),
                ));

                app_proto_selected = true;
            }
        }

        // If we have an HTTP connection, first issue the requests then
        // process received data.
        if let Some(h_conn) = http_conn.as_mut() {
            h_conn.send_requests(&mut conn, &args.dump_response_path);
            h_conn.handle_responses(&mut conn, &mut buf, &app_data_start);
        }

        // Handle path events.
        while let Some(qe) = conn.path_event_next() {
            match qe {
                quiche::PathEvent::New(..) => unreachable!(),

                quiche::PathEvent::Validated(local_addr, peer_addr) => {
                    info!(
                        "Path ({}, {}) is now validated",
                        local_addr, peer_addr
                    );
                    conn.migrate(local_addr, peer_addr).unwrap();
                    migrated = true;
                },

                quiche::PathEvent::FailedValidation(local_addr, peer_addr) => {
                    info!(
                        "Path ({}, {}) failed validation",
                        local_addr, peer_addr
                    );
                },

                quiche::PathEvent::Closed(local_addr, peer_addr) => {
                    info!(
                        "Path ({}, {}) is now closed and unusable",
                        local_addr, peer_addr
                    );
                },

                quiche::PathEvent::ReusedSourceConnectionId(
                    cid_seq,
                    old,
                    new,
                ) => {
                    info!(
                        "Peer reused cid seq {} (initially {:?}) on {:?}",
                        cid_seq, old, new
                    );
                },

                quiche::PathEvent::PeerMigrated(..) => unreachable!(),
            }
        }

        // See whether source Connection IDs have been retired.
        while let Some(retired_scid) = conn.retired_scid_next() {
            info!("Retiring source CID {:?}", retired_scid);
        }

        // Provides as many CIDs as possible.
        while conn.source_cids_left() > 0 {
            let (scid, reset_token) = generate_cid_and_reset_token(&rng);

            if conn.new_source_cid(&scid, reset_token, false).is_err() {
                break;
            }

            scid_sent = true;
        }


        // test_ip = local_ip_address::local_ip().unwrap();

        network_interfaces = list_afinet_netifas().unwrap();
        for (name, ip) in network_interfaces.iter() {
            if name == "enp0s8" {
                test_ip = *ip;
            }
            println!("name: {} ip: {}", name, ip);
        }
        
        // Below is for an incomplete implementation of "simple" IP migration support -- TODO
        // consume a connection ID
        if test_ip != local_ip && enable_ip_migration {
            println!("test_ip not equal to local! {:?} {:?} ", test_ip, local_ip);
            local_ip = test_ip;
            println!("test_ip is now: {:?}", test_ip);
            let mut new_ip_socket = mio::net::UdpSocket::bind(bind_addr.parse().unwrap()).unwrap();
            poll.registry().register(&mut new_ip_socket, mio::Token(2), mio::Interest::READABLE).unwrap();
            ip_mig_socket = Some(new_ip_socket);
            ip_migrate_bool = true;
            // TODO figure out how to retire a destination CID
            // This will be done by defining a new lib.rs function to 
            // retire "active" destination cid (destination id of active path)
            
            // See lib.rs:5764 pub fn retire_destination_cid
        }

        if args.perform_migration &&
            !new_path_probed &&
            scid_sent &&
            conn.available_dcids() > 0
        {
            println!("probing path");
            let additional_local_addr =
                migrate_socket.as_ref().unwrap().local_addr().unwrap();
            conn.probe_path(additional_local_addr, peer_addr).unwrap();

            new_path_probed = true;
        }

        // Below is for adding additional timely path probing to IP migration support
        println!("migrate args: bool {} not probed {} sent {} avail {} enable {} enhanced {}", ip_migrate_bool, !new_path_probed, scid_sent, conn.available_dcids(), enable_ip_migration, enable_enhanced_ip_mig);
        if ip_migrate_bool &&
            !new_path_probed &&
            scid_sent &&
            conn.available_dcids() > 0 &&
            enable_ip_migration &&
            enable_enhanced_ip_mig
        {
            println!("got inside loop");
            let new_local_addr =
                ip_mig_socket.as_ref().unwrap().local_addr().unwrap();
            conn.probe_path(new_local_addr, peer_addr).unwrap();
            new_path_probed = true;
        }

        // Generate outgoing QUIC packets and send them on the UDP socket, until
        // quiche reports that there are no more packets to be sent.
        let mut sockets = vec![&socket];
        if let Some(migrate_socket) = migrate_socket.as_ref() {
            sockets.push(migrate_socket);
        }

        if let Some(ip_mig_socket) = ip_mig_socket.as_ref() {
            sockets.push(ip_mig_socket);
        }

        for socket in sockets {
            let local_addr = socket.local_addr().unwrap();

            for peer_addr in conn.paths_iter(local_addr) {
                loop {
                    let (write, send_info) = match conn.send_on_path(
                        &mut out,
                        Some(local_addr),
                        Some(peer_addr),
                    ) {
                        Ok(v) => v,

                        Err(quiche::Error::Done) => {
                            trace!(
                                "{} -> {}: done writing",
                                local_addr,
                                peer_addr
                            );
                            break;
                        },

                        Err(e) => {
                            error!(
                                "{} -> {}: send failed: {:?}",
                                local_addr, peer_addr, e
                            );

                            conn.close(false, 0x1, b"fail").ok();
                            break;
                        },
                    };

                    if let Err(e) = socket.send_to(&out[..write], send_info.to) {
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            trace!(
                                "{} -> {}: send() would block",
                                local_addr,
                                send_info.to
                            );
                            break;
                        }

                        return Err(ClientError::Other(format!(
                            "{} -> {}: send() failed: {:?}",
                            local_addr, send_info.to, e
                        )));
                    }

                    trace!(
                        "{} -> {}: written {}",
                        local_addr,
                        send_info.to,
                        write
                    );
                }
            }
        }

        if conn.is_closed() {
           // Join the IP change detection thread
           //handle.join().unwrap();
           stop.store(true, Ordering::Relaxed);
           info!("Stop set to True -- thread should terminate");

           info!(
                "connection closed, {:?} {:?}",
                conn.stats(),
                conn.path_stats().collect::<Vec<quiche::PathStats>>()
            );

            if !conn.is_established() {
                error!(
                    "connection timed out after {:?}",
                    app_data_start.elapsed(),
                );

                return Err(ClientError::HandshakeFail);
            }

            if let Some(session_file) = &args.session_file {
                if let Some(session) = conn.session() {
                    std::fs::write(session_file, session).ok();
                }
            }

            if let Some(h_conn) = http_conn {
                if h_conn.report_incomplete(&app_data_start) {
                    return Err(ClientError::HttpFail);
                }
            }

            break;
        }
    }

    Ok(())
}
