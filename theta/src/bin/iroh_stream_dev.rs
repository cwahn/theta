// use iroh::{Endpoint, NodeAddr, RelayUrl, Watcher};
// use std::collections::BTreeSet;
// use std::io::{self, Write};
// use std::net::SocketAddr;
// use std::time::Duration;
// use tokio::io::AsyncWriteExt;

// #[tokio::main]
// async fn main() -> Result<(), Box<dyn std::error::Error>> {
//     // Create endpoint with discovery services
//     let endpoint = Endpoint::builder()
//         .alpns(vec![b"iroh-counter".to_vec()])
//         .discovery_n0()
//         .bind()
//         .await?;

//     let node_id = endpoint.node_id();
//     println!("Node ID: {node_id}");

//     // Wait for node address to be available
//     println!("Initializing node...");
//     let mut node_addr = None;
//     for _ in 0..10 {
//         if let Ok(Some(addr)) = endpoint.node_addr().get() {
//             node_addr = Some(addr);
//             break;
//         }
//         tokio::time::sleep(Duration::from_millis(100)).await;
//     }

//     match &node_addr {
//         Some(addr) => {
//             println!("Node Address: {addr:?}");
//             println!("Copy this FULL address to connect from another node");
//         }
//         None => {
//             println!("Warning: Node address still not available, but continuing...");
//         }
//     }

//     println!("\nOptions:");
//     println!("1. Press Enter to wait for incoming connections");
//     println!("2. Enter just the NodeID to try discovery-based connection");
//     println!("3. Enter the full NodeAddr to connect directly");
//     println!();
//     println!("For best results on local network, copy the FULL NodeAddr line:");
//     if let Some(addr) = &node_addr {
//         println!(
//             "Example: NodeAddr {{ node_id: PublicKey({}), relay_url: {:?}, direct_addresses: {:?} }}",
//             addr.node_id, addr.relay_url, addr.direct_addresses
//         );
//     }

//     // Read user input for peer address
//     print!("> ");
//     io::stdout().flush()?;
//     let mut input = String::new();
//     io::stdin().read_line(&mut input)?;
//     let input = input.trim();

//     if input.is_empty() {
//         // Act as server - wait for incoming connections
//         println!("Waiting for incoming connections...");
//         run_server(endpoint).await?;
//     } else if input.starts_with("NodeAddr") {
//         // Parse full NodeAddr
//         println!("Parsing full NodeAddr...");
//         if let Some(peer_addr) = parse_node_addr(input) {
//             run_client(endpoint, peer_addr).await?;
//         } else {
//             println!("Failed to parse NodeAddr. Please copy the exact output.");
//             return Ok(());
//         }
//     } else {
//         // Try to parse as just NodeID
//         println!("Connecting to peer using NodeID only...");
//         match input.parse::<iroh::PublicKey>() {
//             Ok(node_id) => {
//                 println!("Parsed NodeID: {node_id}");
//                 println!("Note: This will likely fail without relay servers or discovery");
//                 let peer_addr = NodeAddr::from(node_id);
//                 run_client(endpoint, peer_addr).await?;
//             }
//             Err(e) => {
//                 println!("Invalid NodeID format: {e}");
//                 return Ok(());
//             }
//         }
//     }

//     Ok(())
// }

// fn parse_node_addr(input: &str) -> Option<NodeAddr> {
//     // Extract node_id
//     let node_id = if let Some(start) = input.find("PublicKey(") {
//         if let Some(end) = input[start..].find(')') {
//             let key_str = &input[start + 10..start + end];
//             key_str.parse::<iroh::PublicKey>().ok()?
//         } else {
//             return None;
//         }
//     } else {
//         return None;
//     };

//     // Extract relay_url (currently always None in your case)
//     let relay_url: Option<RelayUrl> = None;

//     // Extract direct_addresses
//     let mut direct_addresses = BTreeSet::new();
//     if let Some(start) = input.find("direct_addresses: {") {
//         if let Some(end) = input[start..].find('}') {
//             let addr_str = &input[start + 19..start + end];
//             if !addr_str.is_empty() && addr_str != " " {
//                 if let Ok(socket_addr) = addr_str.parse::<SocketAddr>() {
//                     direct_addresses.insert(socket_addr);
//                 }
//             }
//         }
//     }

//     // Create NodeAddr using the from tuple constructor
//     Some(NodeAddr::from((
//         node_id,
//         relay_url,
//         direct_addresses
//             .iter()
//             .cloned()
//             .collect::<Vec<_>>()
//             .as_slice(),
//     )))
// }

// async fn run_server(endpoint: Endpoint) -> Result<(), Box<dyn std::error::Error>> {
//     while let Some(incoming) = endpoint.accept().await {
//         match incoming.await {
//             Ok(connection) => {
//                 println!("Accepted connection from peer");

//                 // Handle multiple streams on this connection
//                 tokio::spawn(async move {
//                     if let Err(e) = handle_multiple_streams_server(connection).await {
//                         println!("Connection error: {e}");
//                     }
//                 });
//                 break;
//             }
//             Err(e) => {
//                 println!("Error accepting connection: {e}");
//                 continue;
//             }
//         }
//     }

//     Ok(())
// }

// async fn run_client(
//     endpoint: Endpoint,
//     peer_addr: NodeAddr,
// ) -> Result<(), Box<dyn std::error::Error>> {
//     // Connect to peer with longer timeout to allow for discovery
//     println!("Attempting to connect to peer...");
//     println!("Peer address: {peer_addr:?}");
//     println!("This may take a moment for discovery to work...");

//     let connection = tokio::time::timeout(
//         Duration::from_secs(30),
//         endpoint.connect(peer_addr, b"iroh-counter"),
//     )
//     .await??;

//     println!("Connected successfully!");

//     // Handle multiple streams as client
//     handle_multiple_streams_client(connection).await?;

//     Ok(())
// }

// async fn handle_multiple_streams_server(
//     connection: iroh::endpoint::Connection,
// ) -> Result<(), Box<dyn std::error::Error>> {
//     println!("Server: Ready to accept multiple streams...");

//     // Accept multiple incoming streams
//     loop {
//         tokio::select! {
//             // Accept new bidirectional streams
//             result = connection.accept_bi() => {
//                 match result {
//                     Ok((send_stream, recv_stream)) => {
//                         println!("Server: New stream accepted");

//                         // Spawn a task to handle this specific stream
//                         let _connection_clone = connection.clone();
//                         tokio::spawn(async move {
//                             if let Err(e) = handle_single_stream(send_stream, recv_stream, false).await {
//                                 println!("Stream error: {e}");
//                             }
//                         });
//                     }
//                     Err(e) => {
//                         println!("Error accepting stream: {e}");
//                         break;
//                     }
//                 }
//             }

//             // Also accept unidirectional streams if needed
//             result = connection.accept_uni() => {
//                 match result {
//                     Ok(_recv_stream) => {
//                         println!("Server: New unidirectional stream accepted");
//                         // Handle unidirectional stream if needed
//                     }
//                     Err(e) => {
//                         println!("Error accepting uni stream: {e}");
//                     }
//                 }
//             }
//         }
//     }

//     Ok(())
// }

// async fn handle_multiple_streams_client(
//     connection: iroh::endpoint::Connection,
// ) -> Result<(), Box<dyn std::error::Error>> {
//     println!("Client: Creating multiple streams...");

//     // Create multiple streams concurrently
//     let mut handles = vec![];

//     for stream_id in 0..3 {
//         let connection_clone = connection.clone();
//         let handle = tokio::spawn(async move {
//             match connection_clone.open_bi().await {
//                 Ok((send_stream, recv_stream)) => {
//                     println!("Client: Opened stream {stream_id}");
//                     if let Err(e) = handle_single_stream(send_stream, recv_stream, true).await {
//                         println!("Stream {stream_id} error: {e}");
//                     }
//                 }
//                 Err(e) => {
//                     println!("Failed to open stream {stream_id}: {e}");
//                 }
//             }
//         });
//         handles.push(handle);
//     }

//     // Wait for all streams to complete
//     for handle in handles {
//         let _ = handle.await;
//     }

//     // Keep connection alive for a bit to see all messages
//     tokio::time::sleep(Duration::from_secs(2)).await;

//     // Close connection gracefully
//     connection.close(0u32.into(), b"session_complete");
//     println!("Client: All streams completed, connection closed");

//     Ok(())
// }

// async fn handle_single_stream(
//     mut send_stream: iroh::endpoint::SendStream,
//     mut recv_stream: iroh::endpoint::RecvStream,
//     is_client: bool,
// ) -> Result<(), Box<dyn std::error::Error>> {
//     let stream_name = if is_client { "Client" } else { "Server" };
//     println!("{stream_name}: Stream handler started");

//     if is_client {
//         // Client sends a message first
//         let message = "Hello from client stream!".to_string();
//         send_stream.write_all(message.as_bytes()).await?;
//         send_stream.flush().await?;
//         println!("{stream_name}: Sent: {message}");

//         // Then wait for response
//         let data = recv_stream.read_to_end(1024).await?;
//         let response = String::from_utf8_lossy(&data);
//         println!("{}: Received: {}", stream_name, response.trim());
//     } else {
//         // Server waits for message first
//         let data = recv_stream.read_to_end(1024).await?;
//         if !data.is_empty() {
//             let message = String::from_utf8_lossy(&data);
//             println!("{}: Received: {}", stream_name, message.trim());

//             // Send response
//             let response = "Hello back from server stream!".to_string();
//             send_stream.write_all(response.as_bytes()).await?;
//             send_stream.flush().await?;
//             println!("{stream_name}: Sent: {response}");
//         }
//     }

//     // Close this stream
//     send_stream.finish()?;
//     println!("{stream_name}: Stream completed");

//     Ok(())
// }
