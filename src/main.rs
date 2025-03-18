use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;
use std::env;
use std::time::Duration;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;

use crossbeam_channel as cbc;

use driver_rust::elevio;
use driver_rust::elevio::elev as e;
use driver_rust::elevio::elev::Elevator;
use driver_rust::network::p2p_connect;
use driver_rust::elevio::cost::{calculate_cost, ElevatorMessage};

/*
// Decide direction based on call received
fn direction_call(go_floor: u8, floor: u8) -> u8 {
    let dirn: u8;

    if floor < go_floor {
        dirn = e::DIRN_UP;
        println!("Going up");
    } else if floor > go_floor {
        dirn = e::DIRN_DOWN;
        println!("Going down");
    } else {
        dirn = e::DIRN_STOP;
    }

    dirn
} */

// Structure to hold shared state for all elevators in the system
struct ElevatorSystem {
    // Local elevator state
    local_id: String,
    local_elevator: Arc<Mutex<Elevator>>,
    
    // Global system state
    hall_calls: Arc<Mutex<HashMap<(u8, u8), (String, u64)>>>, // (floor, direction) -> (assigned_to, timestamp)
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
    
    // Assign a hall call to the best elevator
    // Replace the assign_hall_call method with this improved version
// that properly handles ties in cost calculation

    fn assign_hall_call(&self, floor: u8, direction: u8, timestamp: u64) {
        {
            let hall_calls = self.hall_calls.lock().unwrap();
            if let Some((assigned_id, existing_ts)) = hall_calls.get(&(floor, direction)) {
                // If already assigned to someone, and no "newer" timestamp, do nothing.
                if !assigned_id.is_empty() && *existing_ts == timestamp {
                    println!(
                        "Call (floor {}, dir {}) is already assigned to {} with same timestamp, skipping.",
                        floor, direction, assigned_id
                    );
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
            
            println!("Call (floor {}, dir {}) assigned to {} (costs: {:?})", 
                floor, direction, best_id, all_costs);
                
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
                start_elevator(&mut elevator);
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
    
    // Process a message received from another elevator
    fn process_message(&self, message: ElevatorMessage, from_addr: Option<String>) {
        // If we got this message from a specific address, make sure it's in our peer list
        if let Some(addr) = from_addr {
            self.add_peer(addr);
        }
        
        match message {
            ElevatorMessage::HallCall { floor, direction, timestamp } => {
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
            },
            ElevatorMessage::ElevatorState { id, floor, direction, call_buttons } => {
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
            },
            ElevatorMessage::CompletedCall { floor, direction } => {
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
fn start_elevator(elevator: &mut Elevator) {
    if elevator.call_buttons.is_empty() || elevator.current_direction != e::DIRN_STOP {
        return;
    }
    
    // Get the first call in the queue
    let first_call = &elevator.call_buttons[0];
    let go_floor = first_call[0];
    let current_floor = elevator.current_floor;
    
    // Determine direction
    let direction = if current_floor < go_floor {
        println!("Going up");
        e::DIRN_UP
    } else if current_floor > go_floor {
        println!("Going down");
        e::DIRN_DOWN
    } else {
        e::DIRN_STOP
    };
    
    // Update direction and start motor
    elevator.current_direction = direction;
    elevator.motor_direction(direction);
}

// Function to handle calls when the elevator reaches a certain floor
fn serve_call(elevator_system: &ElevatorSystem, floor: u8) {
    let mut elevator = elevator_system.local_elevator.lock().unwrap();
    
    // If no calls to serve, return
    if elevator.call_buttons.is_empty() {
        return;
    }
    
    // Get first call in queue
    let serving_call = elevator.call_buttons[0][1];
    
    // Check if there is a call for this floor
    if let Some(pos) = elevator.call_buttons.iter().position(|call|
        call[0] == floor && (call[1] == serving_call || call[1] == e::CAB)) {
        
        // Stop the elevator
        elevator.motor_direction(e::DIRN_STOP);
        elevator.current_direction = e::DIRN_STOP;
        
        // Turn off the call button light
        let call_type = elevator.call_buttons[pos][1];
        elevator.call_button_light(floor, call_type, false);
        
        // If this was a hall call, mark it as completed
        if call_type != e::CAB {
            // Need to drop the elevator lock before acquiring system locks
            let completed_direction = call_type;
            drop(elevator);
            elevator_system.complete_call(floor, completed_direction);
            elevator = elevator_system.local_elevator.lock().unwrap();
        }
        
        // Remove call from queue
        elevator.call_buttons.remove(pos);
        println!("Call for floor {} removed", floor);
        
        // Open door for 3 seconds
        elevator.door_light(true);
        std::thread::sleep(Duration::from_secs(3));
        elevator.door_light(false);
        
        // Sort call queue to prioritize cab calls
        elevator.call_buttons.sort_by_key(|call| if call[1] == e::CAB { 0 } else { 1 });
        
        // Decide next direction based on remaining calls
        if !elevator.call_buttons.is_empty() {
            let next_floor = elevator.call_buttons[0][0];
            
            let new_dir = if next_floor > floor {
                e::DIRN_UP
            } else if next_floor < floor {
                e::DIRN_DOWN
            } else {
                // If next call is for the current floor, serve it immediately
                let next_call_type = elevator.call_buttons[0][1]; 
                elevator.call_button_light(floor, next_call_type, false);
                
                // If hall call, mark as completed
                if next_call_type != e::CAB {
                    // Need to drop the elevator lock before acquiring system locks
                    drop(elevator);
                    elevator_system.complete_call(floor, next_call_type);
                    elevator = elevator_system.local_elevator.lock().unwrap();
                }
                
                elevator.call_buttons.remove(0);
                e::DIRN_STOP
            };
            
            // Update elevator direction and motor
            elevator.current_direction = new_dir;
            elevator.motor_direction(new_dir);
        }
    }
}

// Custom message listener thread
fn message_listener(elevator_system: Arc<ElevatorSystem>, port: u16) {
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
    let elevator = e::Elevator::init(&server_addr, elev_num_floors)?;
    
    // Initialize network
    let network_port = 7878 + elev_port - 15657; // Network ports start at 7878
    let network_manager = p2p_connect::start_peer_manager(network_port);
    
    // Initialize elevator system
    let elevator_system = Arc::new(ElevatorSystem::new(
        elev_id.clone(),
        elevator.clone(),
        Arc::clone(&network_manager)
    ));
    
    // Start message listener
    let message_port = 8878 + elev_port - 15657; // Message ports start at 8878
    message_listener(Arc::clone(&elevator_system), message_port);
    
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
                    start_elevator(&mut elevator);
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