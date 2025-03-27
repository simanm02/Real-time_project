use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::time::{SystemTime, Duration};
use std::io::{Read, Write};
use std::thread;
// use std::net::{TcpStream, TcpListener};
use std::net::TcpListener;
use crossbeam_channel as cbc;

use crate::elevio::elev::Elevator;
use crate::elevio::cost::{calculate_cost, ElevatorMessage};
use crate::elevio::fault_handler;
use crate::network::p2p_connect;
use crate::control::{
    start_elevator, 
    direction_to_string
};

// Structure to hold shared state for all elevators in the system
pub struct ElevatorSystem {
    // Local elevator state
    pub local_id: String,
    pub local_elevator: Arc<Mutex<Elevator>>,
    
    // Global system state
    pub hall_calls: Arc<Mutex<HashMap<(u8, u8), (String, u64)>>>, // (floor, direction) -> (aigned_to,ss timestamp)
    pub elevator_states: Arc<Mutex<HashMap<String, ElevatorState>>>,
    
    // Network communication
    pub network_manager: Arc<p2p_connect::NetworkManager>,
    pub peers: Arc<Mutex<Vec<String>>>, // Store peer addresses separately
}

// Structure to keep track of other elevator states
#[derive(Clone, Debug)]
pub struct ElevatorState {
    pub floor: u8,
    pub direction: u8,
    pub call_buttons: Vec<Vec<u8>>,
    pub last_seen: u64, // Timestamp of last update
    pub is_obstructed: bool,
    pub obstruction_duration: u64,
}

pub fn start_reconnection_service(elevator_system: Arc<ElevatorSystem>) {
    thread::spawn(move || {
        loop {
            // Sleep for 5 seconds between reconnection attempts
            thread::sleep(Duration::from_secs(5));
                
            // Try to reconnect to potential peers
            for i in 0..3 {
                let elevator_id = elevator_system.local_id.parse::<usize>().unwrap_or(0);
                if i as usize != elevator_id - 1 {
                    let peer_message_port = 8878 + i;
                    let peer_addr = format!("localhost:{}", peer_message_port);
                        
                    // Try to establish bidirectional connection
                    elevator_system.establish_bidirectional_connection(&peer_addr);
                }
            }
        }
    });
}

// Custom message listener thread
pub fn message_listener(elevator_system: Arc<ElevatorSystem>, port: u16, fault_monitor: Arc<Mutex<fault_handler::ElevatorHealthMonitor>>) {
    // Create a TCP listener for direct message passing
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).expect("Could not bind to address");
    println!("Message listener started on port {}", port);
    
    // Create a channel for passing messages to the processor
    let (tx, rx) = cbc::unbounded::<(String, String)>(); // (message, from_addr)
    
    // Accept connections and handle messages
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let addr = match stream.peer_addr() {
                        Ok(a) => a.to_string(),
                        Err(_) => continue,
                    };
                    
                    let tx_clone = tx.clone();
                    
                    thread::spawn(move || {
                        let mut buffer = [0; 1024];
                        
                        loop {
                            match stream.read(&mut buffer) {
                                Ok(0) => break, // Connection closed
                                Ok(n) => {
                                    let message = String::from_utf8_lossy(&buffer[..n]).to_string();
                                    tx_clone.send((message, addr.clone())).unwrap_or(());
                                },
                                Err(_) => break,
                            }
                        }
                    });
                },
                Err(e) => eprintln!("Error accepting connection: {}", e),
            }
        }
    });
    
    // Process received messages
    let elevator_system_clone = Arc::clone(&elevator_system);
    thread::spawn(move || {
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok((message, from_addr)) => {
                    if let Some(parsed_msg) = ElevatorMessage::from_string(&message) {

                        if let ElevatorMessage::ElevatorState { ref id, .. } = parsed_msg {
                            fault_monitor.lock().unwrap().record_heartbeat(id);
                        }
                        elevator_system_clone.process_message(parsed_msg, Some(from_addr));
                    }
                },
                Err(_) => {
                    // Timeout - check for disconnected elevators
                    elevator_system_clone.check_disconnected_elevators();
                    
                    // Broadcast our state periodically
                    elevator_system_clone.broadcast_state();
                }
            }
        }
    });
}

impl ElevatorSystem {
    pub fn new(
        local_id: String, 
        elevator: Elevator, 
        network_manager: Arc<p2p_connect::NetworkManager>
    ) -> Self {
        let local_elevator = Arc::new(Mutex::new(elevator));
        
        ElevatorSystem {
            local_id,
            local_elevator,
            hall_calls: Arc::new(Mutex::new(HashMap::new())),
            elevator_states: Arc::new(Mutex::new(HashMap::new())),
            network_manager,
            peers: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    // Add a peer to our list
    pub fn add_peer(&self, addr: String) {
        let mut peers = self.peers.lock().unwrap();
        if !peers.contains(&addr) {
            peers.push(addr);
        }
    }
    
    
    pub fn establish_bidirectional_connection(&self, peer_addr: &str) -> bool {
        // 1. Try to connect first to verify peer is available
        match std::net::TcpStream::connect(peer_addr) {
            Ok(mut stream) => {
                // Peer is available, proceed with connection
                self.add_peer(peer_addr.to_string());
                p2p_connect::connect(Arc::clone(&self.network_manager), peer_addr);
                
                // Send our state 
                let elevator = self.local_elevator.lock().unwrap();
                let msg = ElevatorMessage::ElevatorState {
                    id: self.local_id.clone(),
                    floor: elevator.current_floor,
                    direction: elevator.current_direction,
                    call_buttons: elevator.call_buttons.clone(),
                    is_obstructed: elevator.is_obstructed,
                };
                
                if stream.write_all(msg.to_string().as_bytes()).is_ok() {
                    println!("Successfully established connection with {}", peer_addr);
                    return true;
                }
            },
            Err(e) => {
                // Don't spam with connection errors, just return false
                return false;
            }
        }
        false
    }

    pub fn handle_sync_request(&self, requesting_id: String) {
        // Send all current hall calls to the requesting elevator
        let hall_calls = self.hall_calls.lock().unwrap().clone();
        
        for ((floor, direction), (assigned_to, timestamp)) in hall_calls {
            let sync_msg = ElevatorMessage::HallCall {
                floor,
                direction,
                timestamp,
            };
            
            // Find the address for this elevator ID
            let peers = self.peers.lock().unwrap();
            for peer_addr in &*peers {
                // Here we're sending to all peers, but ideally would target just the requesting elevator
                p2p_connect::send(Arc::clone(&self.network_manager), peer_addr, &sync_msg.to_string());
            }
        }
    }
    
    // Broadcast a message to all peers
    pub fn broadcast_message(&self, message: &str) {
        let peers = self.peers.lock().unwrap();
        for addr in &*peers {
            p2p_connect::send(Arc::clone(&self.network_manager), addr, message);
        }
    }
    
    // Broadcast local elevator state to all peers
    pub fn broadcast_state(&self) {
        let elevator = self.local_elevator.lock().unwrap();
        
        let msg = ElevatorMessage::ElevatorState {
            id: self.local_id.clone(),
            floor: elevator.current_floor,
            direction: elevator.current_direction,
            call_buttons: elevator.call_buttons.clone(),
            is_obstructed: elevator.is_obstructed,
        };
        
        self.broadcast_message(&msg.to_string());
    }

    pub fn handle_hall_call_message(&self, floor: u8, direction: u8, timestamp: u64) {
        // Add to hall calls if newer than what we have
        let mut update_needed = false;
        {
            let mut hall_calls = self.hall_calls.lock().unwrap();
            
            if let Some((_, existing_timestamp)) = hall_calls.get(&(floor, direction)) {
                if timestamp > *existing_timestamp {
                    hall_calls.insert((floor, direction), (String::new(), timestamp));
                    update_needed = true;
                }
            } else {
                hall_calls.insert((floor, direction), (String::new(), timestamp));
                update_needed = true;
            }
        }
        
        if update_needed {
            // Turn on the hall call light
            {
                let elevator = self.local_elevator.lock().unwrap();
                elevator.call_button_light(floor, direction, true);
            }
            
            // Determine the best elevator for this call
            self.assign_hall_call(floor, direction, timestamp);
        }
    }


    
    // Process a new hall call
    pub fn process_hall_call(&self, floor: u8, direction: u8) {
        let timestamp = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(n) => n.as_secs(),
            Err(_) => 0,
        };
        
        // Broadcast the hall call to all peers
        let msg = ElevatorMessage::HallCall {
            floor,
            direction,
            timestamp,
        };
        
        // Store the hall call locally
        {
            let mut hall_calls = self.hall_calls.lock().unwrap();
            hall_calls.insert((floor, direction), (String::new(), timestamp)); // Initially unassigned
        }
        
        // Broadcast to all peers
        self.broadcast_message(&msg.to_string());
        
        // Determine the best elevator for this call
        self.assign_hall_call(floor, direction, timestamp);
    }

    pub fn handle_elevator_state_message(&self, id: String, floor: u8, direction: u8, call_buttons: Vec<Vec<u8>>, is_obstructed: bool) {
        // Update our knowledge of this elevator's state
        let mut elevator_states = self.elevator_states.lock().unwrap();
        
        let timestamp = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(n) => n.as_secs(),
            Err(_) => 0,
        };
        
        elevator_states.insert(id.clone(), ElevatorState {
            floor,
            direction,
            call_buttons,
            last_seen: timestamp,
            is_obstructed,
            obstruction_duration: 0,
        });
        
        // Re-evaluate hall call assignments
        drop(elevator_states); // Release the lock before calling assign_hall_call
        
        let hall_calls = self.hall_calls.lock().unwrap().clone();
        for ((call_floor, call_direction), (_, timestamp)) in hall_calls {
            self.assign_hall_call(call_floor, call_direction, timestamp);
        }
    }

    pub fn handle_completed_call_message(&self, floor: u8, direction: u8) {
        println!("Received CompletedCall message for floor {}, direction {}", 
                 floor, direction_to_string(direction));
        
        // Update hall call status
        let was_assigned_to_us = {
            let mut hall_calls = self.hall_calls.lock().unwrap();
            if let Some((assigned_to, _)) = hall_calls.remove(&(floor, direction)) {
                println!("Removed hall call from map. Was assigned to: {}", assigned_to);
                assigned_to == self.local_id
            } else {
                false
            }
        };
        
        // ALWAYS turn off the hall call light, regardless of who it was assigned to
        {
            let elevator = self.local_elevator.lock().unwrap();
            elevator.call_button_light(floor, direction, false);
            println!("Turned off hall call light for floor {}, direction {}", 
                    floor, direction_to_string(direction));
        }
        
        // If it was assigned to us but we didn't complete it, remove from our call queue too
        if was_assigned_to_us {
            let mut elevator = self.local_elevator.lock().unwrap();
            let callbutton = vec![floor, direction];
            
            if let Some(pos) = elevator.call_buttons.iter().position(|x| x == &callbutton) {
                elevator.call_buttons.remove(pos);
                println!("Removed hall call from our queue: floor {}, direction {}", 
                       floor, direction_to_string(direction));
                
                // Persist state after removing the call
                fault_handler::persist_elevator_state(
                    &self.local_id,
                    elevator.current_floor,
                    elevator.current_direction,
                    &elevator.call_buttons
                ).unwrap_or_else(|e| eprintln!("Failed to persist state: {}", e));
            }
        }
    }


    
    // Assign a hall call to the best elevator
    // that properly handles ties in cost calculation

    

    pub fn assign_hall_call(&self, floor: u8, direction: u8, timestamp: u64) {
        // Check if call is already assigned
        {
            let hall_calls = self.hall_calls.lock().unwrap();
            if let Some((assigned_id, existing_ts)) = hall_calls.get(&(floor, direction)) {
                // If already assigned to someone, and no "newer" timestamp, do nothing
                if !assigned_id.is_empty() && *existing_ts == timestamp {
                    // Don't print anything - reduces console spam
                    return;
                }
            }
        }
        
        // Store (cost, id) pairs for all elevators
        let mut all_costs: Vec<(i32, String)> = Vec::new();
        
        // Calculate cost for local elevator
        {
            let elevator = self.local_elevator.lock().unwrap();
            let cost = calculate_cost(
                elevator.current_floor,
                elevator.current_direction,
                elevator.call_buttons.len(),
                floor,
                direction,
                elevator.is_obstructed
            );
            all_costs.push((cost, self.local_id.clone()));
        }
        
        // Calculate costs for other elevators
        {
            let elevator_states = self.elevator_states.lock().unwrap();
            
            for (id, state) in elevator_states.iter() {
                // Calculate cost using state information directly
                let cost = calculate_cost(
                    state.floor,
                    state.direction,
                    state.call_buttons.len(),
                    floor,
                    direction,
                    state.is_obstructed
                );
                all_costs.push((cost, id.clone()));
            }
        }

        if all_costs.is_empty() {
            println!("WARNING: No available elevators to assign call (floor {}, dir {})", 
                     floor, direction_to_string(direction));
            // Still update the hall call as unassigned
            let mut hall_calls = self.hall_calls.lock().unwrap();
            hall_calls.insert((floor, direction), (String::new(), timestamp));
            return;
        }
        
        // Sort by cost (ascending) and then by id (ascending) for consistent tie-breaking
        all_costs.sort_by(|a, b| {
            match a.0.cmp(&b.0) {
                std::cmp::Ordering::Equal => a.1.cmp(&b.1),
                other => other,
            }
        });
        
        // The best elevator is the first in the sorted list
        if let Some((_, best_id)) = all_costs.first() {
            let best_id = best_id.clone();
            
            // Only print if this elevator is the one assigned to handle the call
            if best_id == self.local_id {
                println!("Hall call (floor {}, dir {}) assigned to this elevator", 
                    floor, direction_to_string(direction));
            }
                
            // Update hall call assignment
            {
                let mut hall_calls = self.hall_calls.lock().unwrap();
                hall_calls.insert((floor, direction), (best_id.clone(), timestamp));
            }
            
            // If we are the best elevator, add the call to our queue
            if best_id == self.local_id {
                let mut elevator = self.local_elevator.lock().unwrap();
                
                // Set the call button light
                elevator.call_button_light(floor, direction, true);
                
                // Add to our queue if not already there
                let callbutton = vec![floor, direction];
                if !elevator.call_buttons.iter().any(|x| x == &callbutton) {
                    elevator.call_buttons.push(callbutton);
                }
                
                // Start elevator if needed
                start_elevator(&mut elevator, floor, direction);
            }
        }
    }

    pub fn process_calls_from_disconnected_elevators(&self, disconnected_ids: &[String]) {
        // Step 1: Find all calls assigned to disconnected elevators
        let mut calls_to_reassign = Vec::new();
        {
            let hall_calls = self.hall_calls.lock().unwrap();
            for ((floor, direction), (assigned_to, timestamp)) in hall_calls.iter() {
                if disconnected_ids.contains(assigned_to) {
                    println!(
                        "Found call (floor={}, direction={}) assigned to disconnected elevator {}",
                        floor, direction_to_string(*direction), assigned_to
                    );
                    calls_to_reassign.push((*floor, *direction, *timestamp));
                }
            }
        }
        
        // Step 2: Mark calls as unassigned first
        {
            let mut hall_calls = self.hall_calls.lock().unwrap();
            for (floor, direction, timestamp) in &calls_to_reassign {
                println!(
                    "Marking call (floor={}, direction={}) as unassigned for reassignment",
                    floor, direction_to_string(*direction)
                );
                hall_calls.insert((*floor, *direction), (String::new(), *timestamp));
            }
        }
        
        // Step 3: Reassign each call explicitly
        for (floor, direction, timestamp) in calls_to_reassign {
            println!(
                "Actively reassigning call: floor={}, direction={}",
                floor, direction_to_string(direction)
            );
            self.assign_hall_call(floor, direction, timestamp);
        }
    }
    
    // Handle a completed call
    pub fn complete_call(&self, floor: u8, direction: u8) {
        // Remove from hall calls
        {
            let mut hall_calls = self.hall_calls.lock().unwrap();
            hall_calls.remove(&(floor, direction));
        }
        
        // Broadcast completion to all peers
        let msg = ElevatorMessage::CompletedCall {
            floor,
            direction,
        };
        
        self.broadcast_message(&msg.to_string());
    }
    
    /// Process a message received from another elevator
    pub fn process_message(&self, message: ElevatorMessage, from_addr: Option<String>) {
        // If we got this message from a specific address, make sure it's in our peer list
        if let Some(addr) = from_addr {
            self.add_peer(addr);
        }
        
        match message {
            ElevatorMessage::HallCall { floor, direction, timestamp } => {
                self.handle_hall_call_message(floor, direction, timestamp);
            },
            ElevatorMessage::ElevatorState { id, floor, direction, call_buttons, is_obstructed } => {
                self.handle_elevator_state_message(id, floor, direction, call_buttons, is_obstructed);
            },
            ElevatorMessage::CompletedCall { floor, direction } => {
                self.handle_completed_call_message(floor, direction);
            },
            /* */
            ElevatorMessage::SyncRequest { id } => {
                self.handle_sync_request(id);
            }
        }
    }

    // Check for and handle disconnected elevators
    pub fn check_disconnected_elevators(&self) {
        let current_time = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(n) => n.as_secs(),
            Err(_) => 0,
        };
    
        let timeout_duration = 5; // Match ELEVATOR_TIMEOUT in fault_handler.rs
        let mut disconnected_ids = Vec::new();
    
        // Identify disconnected elevators with more detailed logging
        {
            let elevator_states = self.elevator_states.lock().unwrap();
            for (id, state) in elevator_states.iter() {
                let time_since_last_seen = current_time.saturating_sub(state.last_seen);
                if time_since_last_seen > timeout_duration {
                    println!(
                        "Detected potential disconnect: ID={}, LastSeen={}, TimeSinceLastSeen={}s, Timeout={}s",
                        id, state.last_seen, time_since_last_seen, timeout_duration
                    );
                    disconnected_ids.push(id.clone());
                }
            }
        }
    
        // Process disconnected elevators
        if !disconnected_ids.is_empty() {
            // Remove from elevator states
            {
                let mut elevator_states = self.elevator_states.lock().unwrap();
                for id in &disconnected_ids {
                    if elevator_states.remove(id).is_some() {
                        println!("Elevator {} disconnected and removed from tracking", id);
                    }
                }
            }
    
            // Find and explicitly mark calls for reassignment
            self.process_calls_from_disconnected_elevators(&disconnected_ids);
        }
    }

}