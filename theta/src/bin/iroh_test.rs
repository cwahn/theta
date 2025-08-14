// use iroh::{Endpoint, Watcher};
// use std::time::Duration;

// #[tokio::main]
// async fn main() -> Result<(), Box<dyn std::error::Error>> {
//     // Create first endpoint (server/provider)
//     let endpoint1 = Endpoint::builder()
//         .alpns(vec![b"iroh-example".to_vec()])
//         .discovery_n0()
//         .bind()
//         .await?;

//     let node1_id = endpoint1.node_id();
//     println!("Node 1 ID: {node1_id}");

//     // Create second endpoint (client/consumer)
//     let endpoint2 = Endpoint::builder().discovery_n0().bind().await?;

//     let node2_id = endpoint2.node_id();
//     println!("Node 2 ID: {node2_id}");

//     // Get node1's addressing information
//     // node_addr() returns a Watcher, we need to get its current value
//     let node1_addr_option = endpoint1.node_addr().get()?;
//     if let Some(node1_addr) = node1_addr_option {
//         println!("Node 1 address: {node1_addr:?}");

//         // Give some time for endpoints to initialize
//         tokio::time::sleep(Duration::from_millis(100)).await;

//         // Spawn a task to handle incoming connections on endpoint1
//         let endpoint1_clone = endpoint1.clone();
//         let server_task = tokio::spawn(async move {
//             // Accept incoming connections
//             while let Some(incoming) = endpoint1_clone.accept().await {
//                 match incoming.await {
//                     Ok(conn) => {
//                         println!("Node 1 accepted connection");
//                         match conn.accept_bi().await {
//                             Ok((mut send, mut recv)) => {
//                                 // Read the message - read_to_end returns Vec<u8>
//                                 match recv.read_to_end(1024).await {
//                                     Ok(data) => {
//                                         println!(
//                                             "Node 1 received: {}",
//                                             String::from_utf8_lossy(&data)
//                                         );

//                                         // Echo it back
//                                         let response = b"Hello back from node 1!";
//                                         let _ = send.write_all(response).await;
//                                         let _ = send.finish();
//                                     }
//                                     Err(e) => println!("Error reading data: {e}"),
//                                 }
//                             }
//                             Err(e) => println!("Error accepting stream: {e}"),
//                         }
//                     }
//                     Err(e) => println!("Error accepting connection: {e}"),
//                 }
//                 break; // Only handle one connection for this example
//             }
//         });

//         // Give the server task time to start listening
//         tokio::time::sleep(Duration::from_millis(100)).await;

//         // Connect endpoint2 to endpoint1
//         println!("Node 2 connecting to Node 1...");
//         let connection = endpoint2.connect(node1_addr, b"iroh-example").await?;

//         // Open a bidirectional stream
//         let (mut send_stream, mut recv_stream) = connection.open_bi().await?;

//         // Send a message from node 2 to node 1
//         let message = b"Hello from node 2!";
//         send_stream.write_all(message).await?;
//         send_stream.finish()?;

//         println!("Node 2 sent: {}", String::from_utf8_lossy(message));

//         // Read the response from node 1 - read_to_end returns Vec<u8>
//         let response = recv_stream.read_to_end(1024).await?;
//         println!(
//             "Node 2 received response: {}",
//             String::from_utf8_lossy(&response)
//         );

//         // Close the connection gracefully
//         connection.close(0u32.into(), b"goodbye");

//         // Wait for server task to complete
//         let _ = server_task.await;
//     } else {
//         println!("Failed to get node1 address");
//     }

//     // Close both endpoints
//     endpoint1.close().await;
//     endpoint2.close().await;

//     println!("Connection closed successfully!");

//     Ok(())
// }
]