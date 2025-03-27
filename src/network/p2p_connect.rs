use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// PeerManager: több kapcsolatot kezel egyszerre
/// PeerManager: manages multiple connections simultaneously
pub struct NetworkManager {
    peers: Arc<Mutex<HashMap<String, TcpStream>>>,
}

impl NetworkManager {
    /// Új `PeerManager` létrehozása
    /// Creating a new `PeerManager`
    pub fn new() -> Arc<Self> {
        Arc::new(NetworkManager {
            peers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Új peer hozzáadása
    /// Adding a new peer
    pub fn add_peer(&self, addr: String, stream: TcpStream) {
        self.peers.lock().unwrap().insert(addr, stream);
    }

    /// Üzenet küldése egy adott peernek
    /// Sending a message to a specific peer
    /*
    pub fn send_message(&self, addr: &str, message: &str) {
        if let Some(stream) = self.peers.lock().unwrap().get_mut(addr) {
            stream.write_all(message.as_bytes()).unwrap();
        } 
        /*
        else {
            println!("Peer {} not found!", addr);
        } */
    }

    /// Send a message to all connected peers
    pub fn broadcast_message(&self, message: &str) {
        let peers = self.peers.lock().unwrap();
        
        for (addr, stream) in peers.iter() {
            match stream.try_clone() {
                Ok(mut stream) => {
                    if let Err(e) = stream.write_all(message.as_bytes()) {
                        println!("Failed to send to {}: {}", addr, e);
                    }
                },
                Err(e) => println!("Failed to clone stream for {}: {}", addr, e),
            }
        }
    }
    */

    pub fn send_message(&self, addr: &str, message: &str) {
        let mut peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(stream) = peers.get_mut(addr) {
            if let Err(e) = stream.write_all(message.as_bytes()) {
                println!("Failed to send message to {}: {}. Removing peer.", addr, e);
                peers.remove(addr);
            }
        }
    }
    
    pub fn broadcast_message(&self, message: &str) {
        let mut peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        let mut to_remove = Vec::new();
        for (addr, stream) in peers.iter_mut() {
            if let Err(e) = stream.write_all(message.as_bytes()) {
                println!("Failed to broadcast to {}: {}. Marking for removal.", addr, e);
                to_remove.push(addr.clone());
            }
        }
        for addr in to_remove {
            peers.remove(&addr);
        }
    }
    
    

    /// Bejövő kapcsolatokat kezelő függvény
    /// Function handling incoming connections
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
    /// Connecting to a peer
    pub fn connect_to_peer(peer_manager: Arc<Self>, addr: &str) {
        match TcpStream::connect(addr) {
            Ok(stream) => {
                println!("Connected to peer: {}", addr);
                peer_manager.add_peer(addr.to_string(), stream.try_clone().unwrap());

                let peers_clone = Arc::clone(&peer_manager.peers);
                let stream_clone = stream.try_clone().unwrap();
                // let addr_clone = addr.to_string();

                thread::spawn(move || {
                    NetworkManager::handle_client(stream_clone, peers_clone);
                });
            }
            Err(e) => println!("Failed to connect to {}: {}", addr, e),
        }
    }

    /// Szerver elindítása bejövő kapcsolatokhoz
    /// Starting the server for incoming connections
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
// **Functions for handling PeerManager**
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