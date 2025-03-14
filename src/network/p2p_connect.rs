use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// PeerManager: több kapcsolatot kezel egyszerre
pub struct NetworkManager {
    peers: Arc<Mutex<HashMap<String, TcpStream>>>,
}

impl NetworkManager {
    /// Új `PeerManager` létrehozása
    pub fn new() -> Arc<Self> {
        Arc::new(NetworkManager {
            peers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Új peer hozzáadása
    fn add_peer(&self, addr: String, stream: TcpStream) {
        self.peers.lock().unwrap().insert(addr, stream);
    }

    /// Üzenet küldése egy adott peernek
    pub fn send_message(&self, addr: &str, message: &str) {
        if let Some(stream) = self.peers.lock().unwrap().get_mut(addr) {
            stream.write_all(message.as_bytes()).unwrap();
        } else {
            println!("Peer {} not found!", addr);
        }
    }

    /// Bejövő kapcsolatokat kezelő függvény
    fn handle_client(mut stream: TcpStream, peers: Arc<Mutex<HashMap<String, TcpStream>>>) {
        let mut buffer = [0; 512];
        let addr = stream.peer_addr().unwrap().to_string();

        loop {
            match stream.read(&mut buffer) {
                Ok(0) => {
                    println!("Peer disconnected: {}", addr);
                    peers.lock().unwrap().remove(&addr);
                    break;
                }
                Ok(n) => {
                    let message = String::from_utf8_lossy(&buffer[..n]);
                    println!("[{}] Received: {}", addr, message);
                }
                Err(_) => break,
            }
        }
    }

    /// Peerhez való csatlakozás
    pub fn connect_to_peer(peer_manager: Arc<Self>, addr: &str) {
        match TcpStream::connect(addr) {
            Ok(stream) => {
                println!("Connected to peer: {}", addr);
                peer_manager.add_peer(addr.to_string(), stream.try_clone().unwrap());

                let peers_clone = Arc::clone(&peer_manager.peers);
                let stream_clone = stream.try_clone().unwrap();
                let addr_clone = addr.to_string();

                thread::spawn(move || {
                    NetworkManager::handle_client(stream_clone, peers_clone);
                });
            }
            Err(e) => println!("Failed to connect to {}: {}", addr, e),
        }
    }

    /// Szerver elindítása bejövő kapcsolatokhoz
    pub fn start_listener(peer_manager: Arc<Self>, port: u16) {
        let listener = TcpListener::bind(("0.0.0.0", port)).expect("Could not bind to address");
        let peers_clone = Arc::clone(&peer_manager.peers);

        thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let addr = stream.peer_addr().unwrap().to_string();
                        println!("New peer connected: {}", addr);

                        peer_manager.add_peer(addr.clone(), stream.try_clone().unwrap());
                        let peers_inner = Arc::clone(&peers_clone);

                        thread::spawn(move || {
                            NetworkManager::handle_client(stream, peers_inner);
                        });
                    }
                    Err(e) => eprintln!("Connection failed: {}", e),
                }
            }
        });
    }
}

// **Függvények a PeerManager kezelésére**
pub fn start_peer_manager(listening_port: u16) -> Arc<NetworkManager> {
    let peer_manager = NetworkManager::new();
    NetworkManager::start_listener(Arc::clone(&peer_manager), listening_port);
    peer_manager
}

pub fn connect(peer_manager: Arc<NetworkManager>, addr: &str) {
    NetworkManager::connect_to_peer(peer_manager, addr);
}

pub fn send(peer_manager: Arc<NetworkManager>, addr: &str, message: &str) {
    peer_manager.send_message(addr, message);
}

// fn main() {
//     let peer_manager = start_peer_manager();
//
//     loop {
//         println!("Enter command: [connect <addr> | send <addr> <msg> | exit]");
//         let mut input = String::new();
//         std::io::stdin().read_line(&mut input).unwrap();
//         let parts: Vec<&str> = input.trim().split_whitespace().collect();
//
//         match parts.as_slice() {
//             ["connect", addr] => connect(Arc::clone(&peer_manager), addr),
//             ["send", addr, msg @ ..] => send(Arc::clone(&peer_manager), addr, &msg.join(" ")),
//             ["exit"] => break,
//             _ => println!("Invalid command."),
//         }
//     }
// }
