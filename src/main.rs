use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;
use std::env;
use std::time::Duration;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::sleep;
use crossbeam_channel as cbc;

use driver_rust::elevio;
use driver_rust::elevio::elev as e;
use driver_rust::elevio::elev::Elevator;
use driver_rust::network::p2p_connect;
use driver_rust::elevio::fault_handler;
use driver_rust::elevio::cost::{calculate_cost, ElevatorMessage};

// Structure to hold shared state for all elevators in the system
struct ElevatorSystem {
    // Local elevator state
    local_id: String,
    local_elevator: Arc<Mutex<Elevator>>,
    
    // Global system state
    hall_calls: Arc<Mutex<HashMap<(u8, u8), (String, u64)>>>, // (floor, direction) -> (aigned_to,ss timestamp)
    elevator_states: Arc<Mutex<HashMap<String, ElevatorState>>>,
    
    // Network communication
    network_manager: Arc<p2p_connect::NetworkManager>,
    peers: Arc<Mutex<Vec<String>>>, // Store peer addresses separately
}

// Structure to keep track of other elevator states
#[derive(Clone, Debug)]
struct ElevatorState {
    floor: u8,
    direction: u8,
    call_buttons: Vec<Vec<u8>>,
    last_seen: u64, // Timestamp of last update
}

fn direction_to_string(direction: u8) -> &'static str {
    match direction {
        e::DIRN_UP => "UP",
        e::DIRN_DOWN => "DOWN",
        e::DIRN_STOP => "STOP",
        _ => "UNKNOWN"
    }
}

impl ElevatorSystem {
    fn new(
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
    fn add_peer(&self, addr: String) {
        let mut peers = self.peers.lock().unwrap();
        if !peers.contains(&addr) {
            peers.push(addr);
        }
    }
    
    
    fn establish_bidirectional_connection(&self, peer_addr: &str) {
        // 1. Add the peer to our local list
        self.add_peer(peer_addr.to_string());
        
        // 2. Connect using the network manager
        p2p_connect::connect(Arc::clone(&self.network_manager), peer_addr);
        
        // 3. Send a direct message to ensure the peer knows about us
        match std::net::TcpStream::connect(peer_addr) {
            Ok(mut stream) => {
                let elevator = self.local_elevator.lock().unwrap();
                let msg = ElevatorMessage::ElevatorState {
                    id: self.local_id.clone(),
                    floor: elevator.current_floor,
                    direction: elevator.current_direction,
                    call_buttons: elevator.call_buttons.clone(),
                };
                stream.write_all(msg.to_string().as_bytes()).unwrap_or(());
                
                // 4. Request current hall call state
                let sync_request = ElevatorMessage::SyncRequest {
                    id: self.local_id.clone(),
                };
                stream.write_all(sync_request.to_string().as_bytes()).unwrap_or(());
            },
            Err(e) => {
                println!("Failed to connect directly to peer at {}: {}", peer_addr, e);
            }
        }
    }

    fn handle_sync_request(&self, requesting_id: String) {
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
    fn broadcast_message(&self, message: &str) {
        let peers = self.peers.lock().unwrap();
        for addr in &*peers {
            p2p_connect::send(Arc::clone(&self.network_manager), addr, message);
        }
    }
    
    // Broadcast local elevator state to all peers
    fn broadcast_state(&self) {
        let elevator = self.local_elevator.lock().unwrap();
        
        let msg = ElevatorMessage::ElevatorState {
            id: self.local_id.clone(),
            floor: elevator.current_floor,
            direction: elevator.current_direction,
            call_buttons: elevator.call_buttons.clone(),
        };
        
        self.broadcast_message(&msg.to_string());
    }

    fn handle_hall_call_message(&self, floor: u8, direction: u8, timestamp: u64) {
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
    fn process_hall_call(&self, floor: u8, direction: u8) {
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

    fn handle_elevator_state_message(&self, id: String, floor: u8, direction: u8, call_buttons: Vec<Vec<u8>>) {
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
        });
        
        // Re-evaluate hall call assignments
        drop(elevator_states); // Release the lock before calling assign_hall_call
        
        let hall_calls = self.hall_calls.lock().unwrap().clone();
        for ((call_floor, call_direction), (_, timestamp)) in hall_calls {
            self.assign_hall_call(call_floor, call_direction, timestamp);
        }
    }

    fn handle_completed_call_message(&self, floor: u8, direction: u8) {
        // Update hall call status
        {
            let mut hall_calls = self.hall_calls.lock().unwrap();
            hall_calls.remove(&(floor, direction));
        }
        
        // Turn off the hall call light
        {
            let elevator = self.local_elevator.lock().unwrap();
            elevator.call_button_light(floor, direction, false);
        }
    }


    
    // Assign a hall call to the best elevator
    // that properly handles ties in cost calculation

    

    fn assign_hall_call(&self, floor: u8, direction: u8, timestamp: u64) {
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
                direction
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
                    direction
                );
                all_costs.push((cost, id.clone()));
            }
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

    
    
    // Handle a completed call
    fn complete_call(&self, floor: u8, direction: u8) {
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
    fn process_message(&self, message: ElevatorMessage, from_addr: Option<String>) {
        // If we got this message from a specific address, make sure it's in our peer list
        if let Some(addr) = from_addr {
            self.add_peer(addr);
        }
        
        match message {
            ElevatorMessage::HallCall { floor, direction, timestamp } => {
                self.handle_hall_call_message(floor, direction, timestamp);
            },
            ElevatorMessage::ElevatorState { id, floor, direction, call_buttons } => {
                self.handle_elevator_state_message(id, floor, direction, call_buttons);
            },
            ElevatorMessage::CompletedCall { floor, direction } => {
                self.handle_completed_call_message(floor, direction);
            },
            ElevatorMessage::SyncRequest { id } => {
                self.handle_sync_request(id);
            }
        }
    }
    
    // Check for and handle disconnected elevators
    fn check_disconnected_elevators(&self) {
        let current_time = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(n) => n.as_secs(),
            Err(_) => 0,
        };
        
        let mut disconnected_ids = Vec::new();
        
        // Identify disconnected elevators (those not seen in the last 10 seconds)
        {
            let elevator_states = self.elevator_states.lock().unwrap();
            for (id, state) in elevator_states.iter() {
                if current_time - state.last_seen > 10 {
                    disconnected_ids.push(id.clone());
                }
            }
        }
        
        // Remove disconnected elevators
        if !disconnected_ids.is_empty() {
            let mut elevator_states = self.elevator_states.lock().unwrap();
            for id in &disconnected_ids {
                elevator_states.remove(id);
                println!("Elevator {} disconnected", id);
            }
        }
        
        // Reassign calls from disconnected elevators
        if !disconnected_ids.is_empty() {
            let mut reassign_calls = Vec::new();
            
            {
                let hall_calls = self.hall_calls.lock().unwrap();
                
                for ((floor, direction), (assigned_to, timestamp)) in hall_calls.iter() {
                    if disconnected_ids.contains(assigned_to) {
                        reassign_calls.push((*floor, *direction, *timestamp));
                    }
                }
            }
            
            // Reassign each call
            for (floor, direction, timestamp) in reassign_calls {
                self.assign_hall_call(floor, direction, timestamp);
            }
        }
    }
}

// Function to start elevator movement based on its call queue
fn start_elevator(elevator: &mut Elevator, go_floor: u8,  mut dirn: u8) {
    if elevator.call_buttons.len() == 1 || (elevator.call_buttons.len() > 1 && elevator.current_direction == e::DIRN_STOP) {
        dirn = hall_call_start_dir(go_floor, elevator.current_floor, dirn);
        elevator.current_direction = dirn;
        elevator.motor_direction(dirn);
    }
}
fn decide_next_call(elevator: &mut Elevator) -> Option<&Vec<u8>> {
    if let Some(next_call) = elevator.call_buttons.iter().find(|call| {
        (elevator.current_direction == e::DIRN_UP && call[0] > elevator.current_floor) ||
            (elevator.current_direction == e::DIRN_DOWN && call[0] < elevator.current_floor)
    }) {
        Some(next_call)
    } else if let Some(next_call) = elevator.call_buttons.iter().find(|call| call[0] == elevator.current_floor ){
        // Handle case when no calls in the current direction
        Some(next_call)
    } else {
        elevator.call_buttons.first()
    }

}
fn hall_call_start_dir (go_floor: u8, floor: u8, mut dirn: u8) -> u8 {
    if floor < go_floor {
        dirn = e::DIRN_UP;
    } else if floor > go_floor {
        dirn = e::DIRN_DOWN;
    } else {
        dirn = e::DIRN_STOP;
    }
    println!("Direction: {:#?}", dirn);
    dirn
}

fn decide_direction_by_call(call: u8) -> u8 {
    if call == 0 {
        e::DIRN_UP
    } else if call == 1 {
        e::DIRN_DOWN
    } else {
        e::DIRN_STOP
    }
}
fn decide_call_by_direction(dirn: u8) -> u8 {
    if dirn == e::DIRN_UP {
        0
    } else if dirn == e::DIRN_DOWN {
        1
    } else {
        2
    }
}

fn is_more_request_in_dir(elevator:  Elevator) -> bool {
    elevator.call_buttons.iter().any(|call| {
        (elevator.current_direction == e::DIRN_UP && call[0] > elevator.current_floor) ||
            (elevator.current_direction == e::DIRN_DOWN && call[0] < elevator.current_floor)
    })
}

fn opposite_dir(dirn: u8) -> u8 {
    if dirn == e::DIRN_UP {
        e::DIRN_DOWN
    } else {
        e::DIRN_UP
    }
}

fn find_call_button_index(call_button: Vec<u8>,elevator: &mut Elevator) -> Option<usize> {
    elevator.call_buttons.iter().position(|call| call == &call_button)
}

/*
// Function to handle calls when the elevator reaches a certain floor
fn serve_call(elevator_system: &ElevatorSystem, floor: u8) {
    let mut elevator = elevator_system.local_elevator.lock().unwrap();
    
    // If no calls to serve, return
    if elevator.call_buttons.is_empty() {
        return;
    }

    // Check if there is a call for this floor
    
    if let Some(pos) = elevator.call_buttons.iter().position(|call| call[0] == floor &&
        (decide_direction_by_call(call[1]) == elevator.current_direction || call[1] == e::CAB)
        || !is_more_request_in_dir(elevator.clone())) {

        //find the serving call
        let mut serving_call = vec![3,3];
        if let Some(same_direction_pos) = elevator.call_buttons.iter().position(|call| call[0] == floor &&
            (decide_direction_by_call(call[1]) == elevator.current_direction)) {
            serving_call = elevator.call_buttons.get(same_direction_pos).unwrap().clone();
            elevator.call_button_light(floor,serving_call[1], false);
            elevator.call_buttons.remove(same_direction_pos);
        }
        else if let Some(opp_dir_pos) = elevator.call_buttons.iter().position(|call| !is_more_request_in_dir(elevator.clone())
            && call[0] == floor && decide_direction_by_call(call[1]) == opposite_dir(elevator.current_direction)) {
            serving_call = elevator.call_buttons.get(opp_dir_pos).unwrap().clone();
            elevator.call_button_light(floor,serving_call[1], false);
            elevator.call_buttons.remove(opp_dir_pos);
        }
        //find if there is a cab call too
        if let Some(cab_call_pos) = elevator.call_buttons.iter().position(|call| call[0] == floor && call[1] == e::CAB) {
            elevator.call_button_light(floor, e::CAB, false);
            elevator.call_buttons.remove(cab_call_pos);
        }

        // Stop the elevator
        elevator.motor_direction(e::DIRN_STOP);
        //elevator.current_direction = e::DIRN_STOP;

        let call_type = serving_call[1];
        
        // If this was a hall call, mark it as completed
        if call_type != e::CAB {
            // Need to drop the elevator lock before acquiring system locks
            let completed_direction = call_type;
            drop(elevator);
            elevator_system.complete_call(floor, completed_direction);
            elevator = elevator_system.local_elevator.lock().unwrap();
        }

        println!("Call for floor {} removed", floor);
        
        // Open door for 3 seconds
        elevator.door_light(true);
        std::thread::sleep(Duration::from_secs(3));

        // Handle pending calls and decide next direction ;
        if let Some(next_call) = decide_next_call(&mut *elevator) {

            let next_floor = next_call[0];
            let next_call_type = next_call[1];
            let mut new_dir = if next_floor > floor {
                e::DIRN_UP
            } else if next_floor < floor {
                e::DIRN_DOWN
            } else {
                let next_call_index = find_call_button_index(next_call.clone(), &mut *elevator).unwrap();
                // If hall call, mark as completed
                if next_call_type != e::CAB {
                    // Need to drop the elevator lock before acquiring system locks
                    drop(elevator);
                    elevator_system.complete_call(floor, next_call_type);
                    elevator = elevator_system.local_elevator.lock().unwrap();
                }
                elevator.call_button_light(floor, next_call_type, false);
                elevator.call_buttons.remove(next_call_index);
                e::DIRN_STOP

            };
            if let Some(next_call) = decide_next_call(&mut *elevator) {
                if new_dir == e::DIRN_STOP {
                    new_dir = hall_call_start_dir(next_call[0], floor, new_dir);
                }
            }
            if new_dir == opposite_dir(elevator.current_direction) {
                println!("Opposite direction");
                sleep(Duration::from_secs(3));
            }
            elevator.door_light(false);
            elevator.current_direction = new_dir;
            elevator.motor_direction(new_dir);
        }
        elevator.door_light(false);
    }
}
     */

     fn serve_call(elevator_system: &ElevatorSystem, floor: u8) {
        let mut elevator = elevator_system.local_elevator.lock().unwrap();
    
        // If no calls to serve, return.
        if elevator.call_buttons.is_empty() {
            return;
        }
    
        // Try to find a call with the same direction as the elevator's current direction.
        let serving_call_opt = if let Some(pos) = elevator.call_buttons.iter().position(|call| {
            call[0] == floor && (decide_direction_by_call(call[1]) == elevator.current_direction || call[1] == e::CAB)
        }) {
            // Remove and return this call.
            let call = elevator.call_buttons.remove(pos);
            elevator.call_button_light(floor, call[1], false);
            Some(call)
        } 
        // If none found, check if there is a call with the opposite direction and no pending request in current direction.
        else if !is_more_request_in_dir(elevator.clone()) {
            if let Some(pos) = elevator.call_buttons.iter().position(|call| {
                call[0] == floor && decide_direction_by_call(call[1]) == opposite_dir(elevator.current_direction)
            }) {
                let call = elevator.call_buttons.remove(pos);
                elevator.call_button_light(floor, call[1], false);
                Some(call)
            } else {
                None
            }
        } else {
            None
        };
    
        // If no valid serving call was found, return without further action.
        let serving_call = match serving_call_opt {
            Some(call) => call,
            None => return,
        };
    
        // For cab calls, also check and remove any extra cab call.
        if let Some(cab_call_pos) = elevator.call_buttons.iter().position(|call| call[0] == floor && call[1] == e::CAB) {
            elevator.call_button_light(floor, e::CAB, false);
            elevator.call_buttons.remove(cab_call_pos);
        }
    
        // Stop the elevator.
        elevator.motor_direction(e::DIRN_STOP);
        let call_type = serving_call[1];
    
        // If this was a hall call, mark it as completed.
        if call_type != e::CAB {
            let completed_direction = call_type;
            drop(elevator);
            elevator_system.complete_call(floor, completed_direction);
            elevator = elevator_system.local_elevator.lock().unwrap();
        }
    
        println!("Call for floor {} removed", floor);
    
        // Open door for 3 seconds.
        elevator.door_light(true);
        std::thread::sleep(Duration::from_secs(3));
    
        // Decide next call.
        if let Some(next_call) = decide_next_call(&mut *elevator) {
            let next_floor = next_call[0];
            let next_call_type = next_call[1];
            let mut new_dir = if next_floor > floor {
                e::DIRN_UP
            } else if next_floor < floor {
                e::DIRN_DOWN
            } else {
                let next_call_index = find_call_button_index(next_call.clone(), &mut *elevator).unwrap();
                if next_call_type != e::CAB {
                    drop(elevator);
                    elevator_system.complete_call(floor, next_call_type);
                    elevator = elevator_system.local_elevator.lock().unwrap();
                }
                elevator.call_button_light(floor, next_call_type, false);
                elevator.call_buttons.remove(next_call_index);
                e::DIRN_STOP
            };
            if let Some(next_call) = decide_next_call(&mut *elevator) {
                if new_dir == e::DIRN_STOP {
                    new_dir = hall_call_start_dir(next_call[0], floor, new_dir);
                }
            }
            if new_dir == opposite_dir(elevator.current_direction) {
                println!("Opposite direction");
                sleep(Duration::from_secs(3));
            }
            elevator.door_light(false);
            elevator.current_direction = new_dir;
            elevator.motor_direction(new_dir);
        }
        elevator.door_light(false);
    }

fn start_reconnection_service(elevator_system: Arc<ElevatorSystem>) {
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
fn message_listener(elevator_system: Arc<ElevatorSystem>, port: u16, fault_monitor: Arc<Mutex<fault_handler::ElevatorHealthMonitor>>) {
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

fn main() -> std::io::Result<()> {
    // Get command line arguments
    let args: Vec<String> = env::args().collect();
    
    // Parse elevator ID and port
    let elev_id = if args.len() > 1 { args[1].clone() } else { "1".to_string() };
    let elev_port = if args.len() > 2 { 
        args[2].parse::<u16>().unwrap_or(15657) 
    } else { 
        15657 
    };
    
    let elev_num_floors = 4;
    
    println!("Starting elevator {} on port {}", elev_id, elev_port);
    
    // Initialize the elevator
    let server_addr = format!("localhost:{}", elev_port);
    let mut elevator = e::Elevator::init(&server_addr, elev_num_floors)?;

    if let Some((saved_floor, saved_direction, saved_calls)) = fault_handler::load_elevator_state(&elev_id) {
        println!("Recovered persisted state for elevator {}", elev_id);
        elevator.current_floor = saved_floor;
        elevator.current_direction = saved_direction;
        elevator.call_buttons = saved_calls;
        elevator.floor_indicator(saved_floor);
    }
    
    // Initialize network
    let network_port = 7878 + elev_port - 15657; // Network ports start at 7878
    let network_manager = p2p_connect::start_peer_manager(network_port);
    
    // Initialize elevator system
    /*
    let elevator_system = Arc::new(ElevatorSystem::new(
        elev_id.clone(),
        elevator.clone(),
        Arc::clone(&network_manager)
    ));
    */
    
    // Initialize elevator system
    let elevator_system = Arc::new(ElevatorSystem::new(
        elev_id.clone(),
        elevator.clone(),
        Arc::clone(&network_manager)
    ));

    

    // *** Add this line to broadcast the current state ***
    elevator_system.broadcast_state();


    let fault_monitor = fault_handler::ElevatorHealthMonitor::new();
    fault_handler::start_health_monitoring(Arc::clone(&fault_monitor), Arc::clone(&elevator_system.hall_calls));
    
    start_reconnection_service(Arc::clone(&elevator_system));

    // Start message listener
    let message_port = 8878 + elev_port - 15657; // Message ports start at 8878
    message_listener(Arc::clone(&elevator_system), message_port, Arc::clone(&fault_monitor));
    
    // Try to connect to other potential elevators
    for i in 0..3 {
        if i != (elev_port - 15657) as usize {
            let peer_message_port = 8878 + i;
            let peer_addr = format!("localhost:{}", peer_message_port);
            let elevator_system_clone = Arc::clone(&elevator_system);
    
            thread::spawn(move || {
                // Retry connecting a few times
                for _ in 0..5 {
                    // 1) Attempt connection using p2p_connect’s function
                    p2p_connect::connect(
                        Arc::clone(&elevator_system_clone.network_manager), 
                        &peer_addr
                    );
    
                    // 2) Add the peer to our local ElevatorSystem list (so we know about it)
                    elevator_system_clone.add_peer(peer_addr.clone());
    
                    // 3) Optionally send initial elevator state directly to that peer
                    //    (If you still want to replicate the original handshake logic)
                    match std::net::TcpStream::connect(&peer_addr) {
                        Ok(mut stream) => {
                            let elevator = elevator_system_clone.local_elevator.lock().unwrap();
                            let msg = ElevatorMessage::ElevatorState {
                                id: elevator_system_clone.local_id.clone(),
                                floor: elevator.current_floor,
                                direction: elevator.current_direction,
                                call_buttons: elevator.call_buttons.clone(),
                            };
                            stream.write_all(msg.to_string().as_bytes()).unwrap_or(());
                        },
                        Err(e) => {
                            println!("Failed to connect directly to peer at {}: {}", &peer_addr, e);
                        }
                    }
    
                    // Sleep a bit and retry
                    thread::sleep(Duration::from_secs(1));
                }
            });
        }
    }

    
    // Set up polling
    let poll_period = Duration::from_millis(25);
    
    // Crossbeam for call buttons
    let (call_button_tx, call_button_rx) = cbc::unbounded::<elevio::poll::CallButton>();
    {
        let elevator_clone = elevator.clone();
        thread::spawn(move || elevio::poll::call_buttons(elevator_clone, call_button_tx, poll_period));
    }
    
    // Crossbeam for floor sensor
    let (floor_sensor_tx, floor_sensor_rx) = cbc::unbounded::<u8>();
    {
        let elevator_clone = elevator.clone();
        thread::spawn(move || elevio::poll::floor_sensor(elevator_clone, floor_sensor_tx, poll_period));
    }
    
    // Crossbeam for stop button
    let (stop_button_tx, stop_button_rx) = cbc::unbounded::<bool>();
    {
        let elevator_clone = elevator.clone();
        thread::spawn(move || elevio::poll::stop_button(elevator_clone, stop_button_tx, poll_period));
    }
    
    // Crossbeam for obstruction
    let (obstruction_tx, obstruction_rx) = cbc::unbounded::<bool>();
    {
        let elevator_clone = elevator.clone();
        thread::spawn(move || elevio::poll::obstruction(elevator_clone, obstruction_tx, poll_period));
    }
    
    // Initialize elevator position
    {
        let elevator = elevator_system.local_elevator.lock().unwrap();
        
        // If the elevator isn't on a specific floor, move down until it reaches one
        if elevator.floor_sensor().is_none() {
            elevator.motor_direction(e::DIRN_DOWN);
        }
        
        // Turn off all call button lights at startup
        for call_type in 0..3 {
            for floor in 0..elev_num_floors {
                elevator.call_button_light(floor, call_type, false);
            }
        }
    }
    
    // Move elevator to ground floor at startup
    let mut starting_floor = floor_sensor_rx.recv().unwrap();
    {
        let mut elevator = elevator_system.local_elevator.lock().unwrap();
        while starting_floor != 0 {
            elevator.motor_direction(e::DIRN_DOWN);
            starting_floor = floor_sensor_rx.recv().unwrap();
            elevator.floor_indicator(starting_floor);
        }
        elevator.motor_direction(e::DIRN_STOP);
        elevator.current_floor = 0;
        elevator.floor_indicator(0);
    }
    
    // Main elevator control loop
    loop {
        cbc::select! {
            // Handle call button presses
            recv(call_button_rx) -> button_type => {
                let call_button = button_type.unwrap();
                println!("Call button pressed: {:#?}", call_button);
                
                if call_button.call == e::CAB {
                    // Handle cab call locally
                    let mut elevator = elevator_system.local_elevator.lock().unwrap();
                    elevator.call_button_light(call_button.floor, call_button.call, true);
                    
                    // Add to local queue
                    let callbutton = vec![call_button.floor, call_button.call];
                    if !elevator.call_buttons.iter().any(|x| x == &callbutton) {
                        elevator.call_buttons.push(callbutton);
                    }
                    
                    // Start elevator if needed
                    start_elevator(&mut elevator, call_button.floor, 0);
                } else {
                    // Handle hall call through the elevator system
                    elevator_system.process_hall_call(call_button.floor, call_button.call);
                }
            },
            
            // Handle floor sensor
            recv(floor_sensor_rx) -> floor_sensor_data => {
                let floor = floor_sensor_data.unwrap();
                
                // Update current floor
                {
                    let mut elevator = elevator_system.local_elevator.lock().unwrap();
                    elevator.current_floor = floor;
                    elevator.floor_indicator(floor);
                    println!("Floor: {:#?}", floor);
                }
                
                // Check if we need to serve any calls at this floor
                serve_call(&elevator_system, floor);
                
                // Broadcast updated state
                elevator_system.broadcast_state();
            },
            
            // Handle stop button
            recv(stop_button_rx) -> stop_btn => {
                let stop = stop_btn.unwrap();
                println!("Stop button: {:?}", stop);
                
                if stop {
                    let mut elevator = elevator_system.local_elevator.lock().unwrap();
                    
                    // Immediately stop the elevator
                    elevator.motor_direction(e::DIRN_STOP);
                    elevator.current_direction = e::DIRN_STOP;
                    
                    // Turn off all call button lights
                    for f in 0..elev_num_floors {
                        for c in 0..3 {
                            elevator.call_button_light(f, c, false);
                        }
                    }
                    
                    // Clear pending call requests
                    elevator.call_buttons.clear();
                    
                    // Open the door
                    elevator.door_light(true);
                    std::thread::sleep(Duration::from_secs(3));
                    elevator.door_light(false);
                    
                    // Broadcast updated state
                    drop(elevator);
                    elevator_system.broadcast_state();
                }
            },
            
            // Handle obstruction
            recv(obstruction_rx) -> obstruction => {
                let mut obstr = obstruction.unwrap();
                let mut elevator = elevator_system.local_elevator.lock().unwrap();
                
                if !elevator.floor_sensor().is_none() && obstr {
                    println!("Obstruction: {:#?}", obstr);
                    
                    while obstr {
                        elevator.motor_direction(e::DIRN_STOP);
                        drop(elevator);
                        obstr = obstruction_rx.recv().unwrap();
                        elevator = elevator_system.local_elevator.lock().unwrap();
                    }
                } else {
                    obstr = false;
                    println!("Obstruction: {:#?}", obstr);
                }
            },
        }
    }
}