// use iroh::{Endpoint, NodeAddr, RelayUrl, Watcher};
// use std::collections::BTreeSet;
// use std::io::{self, Write};
// use std::net::SocketAddr;
// use std::time::Duration;
// use tokio::io::AsyncWriteExt;

// #[tokio::main]
// async fn main() -> Result<(), Box<dyn std::error::Error>> {
//     // Create endpoint with discovery services
//     // Local discovery should be enabled by default in recent iroh versions
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
//     // Parse NodeAddr from debug string like:
//     // NodeAddr { node_id: PublicKey(abc123...), relay_url: None, direct_addresses: {192.168.0.86:61755} }

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
//     let mut counter = 0u64;

//     while let Some(incoming) = endpoint.accept().await {
//         match incoming.await {
//             Ok(connection) => {
//                 println!("Accepted connection from peer");

//                 // Handle the connection
//                 if let Err(e) = handle_connection(connection, &mut counter, false).await {
//                     println!("Connection error: {e}");
//                 }
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
//     let mut counter = 0u64;

//     // Connect to peer with longer timeout to allow for discovery
//     println!("Attempting to connect to peer...");
//     println!("Peer address: {peer_addr:?}");
//     println!("This may take a moment for discovery to work...");

//     let connection = tokio::time::timeout(
//         Duration::from_secs(30), // Increased timeout for discovery
//         endpoint.connect(peer_addr, b"iroh-counter"),
//     )
//     .await??;

//     println!("Connected successfully!");

//     // Handle the connection as initiator
//     handle_connection(connection, &mut counter, true).await?;

//     Ok(())
// }

// async fn handle_connection(
//     connection: iroh::endpoint::Connection,
//     counter: &mut u64,
//     is_initiator: bool,
// ) -> Result<(), Box<dyn std::error::Error>> {
//     // Open bidirectional stream
//     let (mut send_stream, mut recv_stream) = if is_initiator {
//         connection.open_bi().await?
//     } else {
//         connection.accept_bi().await?
//     };

//     println!("Stream established. Starting counter exchange...");
//     println!("Current counter: {counter}");

//     // Determine who goes first
//     let mut my_turn = is_initiator;

//     loop {
//         if my_turn {
//             // My turn to send a number
//             println!("\nYour turn! Enter a number to send:");
//             print!("> ");
//             io::stdout().flush()?;

//             let mut input = String::new();
//             io::stdin().read_line(&mut input)?;
//             let input = input.trim();

//             if input == "quit" || input == "exit" {
//                 println!("Goodbye!");
//                 break;
//             }

//             match input.parse::<u64>() {
//                 Ok(number) => {
//                     // Send the number
//                     let message = format!("{number}");
//                     send_stream.write_all(message.as_bytes()).await?;
//                     send_stream.flush().await?;

//                     println!("Sent: {number}");

//                     // Add to our counter
//                     *counter += number;
//                     println!("Counter updated: {counter} (added {number})");

//                     my_turn = false;
//                 }
//                 Err(_) => {
//                     println!("Invalid number, please try again");
//                     continue;
//                 }
//             }
//         } else {
//             // Wait for peer's number
//             println!("\nWaiting for peer's number...");

//             // Use select to handle both reading and potential timeout
//             tokio::select! {
//                 result = recv_stream.read_to_end(1024) => {
//                     match result {
//                         Ok(data) => {
//                             if data.is_empty() {
//                                 println!("Peer disconnected");
//                                 break;
//                             }

//                             let received_data = String::from_utf8_lossy(&data);
//                             let received_data = received_data.trim();

//                             match received_data.parse::<u64>() {
//                                 Ok(number) => {
//                                     println!("Received: {number}");

//                                     // Add to our counter
//                                     *counter += number;
//                                     println!("Counter updated: {counter} (added {number})");

//                                     my_turn = true;
//                                 }
//                                 Err(_) => {
//                                     println!("Received invalid number: {received_data}");
//                                     continue;
//                                 }
//                             }
//                         }
//                         Err(e) => {
//                             println!("Error reading from stream: {e}");
//                             break;
//                         }
//                     }
//                 }
//                 _ = tokio::time::sleep(Duration::from_secs(30)) => {
//                     println!("Timeout waiting for peer. Connection might be lost.");
//                     break;
//                 }
//             }
//         }
//     }

//     // Close connection gracefully
//     connection.close(0u32.into(), b"session_complete");
//     println!("Final counter value: {counter}");

//     Ok(())
// }
