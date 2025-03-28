use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, Duration};
use std::env;
use std::io::Write;
use crossbeam_channel as cbc;

use driver_rust::elevio;
use driver_rust::elevio::elev as e;
use driver_rust::network::p2p_connect;
use driver_rust::elevio::fault_handler;
use driver_rust::elevio::cost::ElevatorMessage;
use driver_rust::elevio::system::{
    ElevatorSystem, 
    start_reconnection_service, 
    message_listener
};
use driver_rust::control::{
    serve_call,
    start_elevator,
    direction_to_string
};




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
    let network_port = 7878 + elev_port - 15657;
    let network_manager = p2p_connect::start_peer_manager(network_port);

    
    // Initialize elevator system
    let elevator_system = Arc::new(ElevatorSystem::new(
        elev_id.clone(),
        elevator.clone(),
        Arc::clone(&network_manager)
    ));

    elevator_system.broadcast_state();


    let fault_monitor = fault_handler::ElevatorHealthMonitor::new();
    fault_handler::start_health_monitoring(
        Arc::clone(&fault_monitor),
         Arc::clone(&elevator_system.hall_calls),
        Arc::clone(&elevator_system));

    {
        let mut monitor = fault_monitor.lock().unwrap();
        monitor.record_heartbeat(&elev_id);
        println!("Registered self (elevator {}) as active", elev_id);
    }
    
    start_reconnection_service(Arc::clone(&elevator_system));

    // Start message listener
    let message_port = 8878 + elev_port - 15657; // Message ports start at 8878
    message_listener(Arc::clone(&elevator_system), message_port, Arc::clone(&fault_monitor));
    
    // Try to connect to other potential elevators
    for i in 0..3 {
        if i != (elev_port - 15657) as usize {


            // ----- Physical machine setup begin here: -----
            let peer_message_port = 8878 ;
            let peer_addr = format!("10.24.139.104:{}", peer_message_port);
            let peer_addr_2 = format!("10.100.23.35:{}", peer_message_port);
            println!("Testing connection to potential peer at {}", peer_addr);
            // ----- Physical machine setup end here -----



            /*
            // ----- Simulator setup begin here: -----
            let peer_message_port = 8878 + i;
            let peer_addr = format!("localhost:{}", peer_message_port);
            // ----- Simulator setup end here -----
            */


            let elevator_system_clone = Arc::clone(&elevator_system);

            let connection_result = elevator_system.establish_bidirectional_connection(&peer_addr);
            println!("Connection test to {} result: {}", peer_addr, connection_result);
    
            thread::spawn(move || {
                // Retry connecting a few times
                for _ in 0..5 {
                    // (Un)Comment out if using physical machines with IP address and not simulator setup:
                    // 1) Attempt connection using connect function in p2p_connect
                    p2p_connect::connect(
                        Arc::clone(&elevator_system_clone.network_manager), 
                        &peer_addr
                    );

                    
                    /*
                    // ----- Simulator setup begin here: -----
                    // 2) Add the peer to our local ElevatorSystem list (so we know about it)
                    elevator_system_clone.add_peer(peer_addr.clone());
                    // ----- Simulator setup end here -----
                     */


                    // (Un)Comment and type correct IP if using physical setup:
                    // ----- Physical machine begin here: -----
                    elevator_system_clone.add_peer(peer_addr.clone());
                    elevator_system_clone.add_peer(peer_addr_2.clone());
                    // ----- Physical machine end here -----


                    // 3) Send our initial state to the peer
                    match std::net::TcpStream::connect(&peer_addr) {
                        Ok(mut stream) => {
                            println!("Connection to {} successful, sending initial state.", peer_addr);
                            let elevator = elevator_system_clone.local_elevator.lock().unwrap();
                            let msg = ElevatorMessage::ElevatorState {
                                id: elevator_system_clone.local_id.clone(),
                                floor: elevator.current_floor,
                                direction: elevator.current_direction,
                                call_buttons: elevator.call_buttons.clone(),
                                is_obstructed: elevator.is_obstructed,
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
                println!("Finished connection attempts for peer: {}", peer_addr);
            });
        }
    }

    
    // Set up polling
    let poll_period = Duration::from_millis(25);
    
    let (call_button_tx, call_button_rx) = cbc::unbounded::<elevio::poll::CallButton>();
    {
        let elevator_clone = elevator.clone();
        thread::spawn(move || elevio::poll::call_buttons(elevator_clone, call_button_tx, poll_period));
    }
    
    let (floor_sensor_tx, floor_sensor_rx) = cbc::unbounded::<u8>();
    {
        let elevator_clone = elevator.clone();
        thread::spawn(move || elevio::poll::floor_sensor(elevator_clone, floor_sensor_tx, poll_period));
    }
    
    let (stop_button_tx, stop_button_rx) = cbc::unbounded::<bool>();
    {
        let elevator_clone = elevator.clone();
        thread::spawn(move || elevio::poll::stop_button(elevator_clone, stop_button_tx, poll_period));
    }
    
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
                        elevator.call_buttons.push(callbutton.clone());

                        println!("Persisting state after adding cab call: floor {}, call {}", call_button.floor, call_button.call);
                        fault_handler::persist_elevator_state(
                            &elevator_system.local_id,
                            elevator.current_floor,
                            elevator.current_direction,
                            &elevator.call_buttons // Pass reference
                        ).unwrap_or_else(|e| eprintln!("Failed to persist state: {}", e));

                    }

                    // Start elevator if needed
                    start_elevator(&mut elevator, callbutton[0], 0); // Use callbutton[0]

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
                    let local_id = elevator_system.local_id.clone();
                    let mut calls_to_reassign = Vec::new();

                    { // Scope for elevator lock
                        let mut elevator = elevator_system.local_elevator.lock().unwrap();

                        // Immediately stop the elevator
                        elevator.motor_direction(e::DIRN_STOP);
                        elevator.current_direction = e::DIRN_STOP;

                        // Turn off all call button lights (local elevator only)
                        for f in 0..elev_num_floors {
                            for c in 0..3 {
                                elevator.call_button_light(f, c, false);
                            }
                        }

                        // Clear local pending call requests (cab calls and assigned hall calls)
                        elevator.call_buttons.clear();

                        // Find hall calls assigned to this elevator
                        {
                            let hall_calls = elevator_system.hall_calls.lock().unwrap();
                            for ((floor, direction), (assigned_to, timestamp)) in hall_calls.iter() {
                                if *assigned_to == local_id {
                                    // Collect floor, direction, and timestamp of calls assigned to this stopped elevator
                                    calls_to_reassign.push((*floor, *direction, *timestamp));
                                    println!("Stop button: Marking call ({}, {}) for reassignment from {}", floor, direction, local_id);
                                }
                            }
                        }


                        // Open the door if at a floor
                        if elevator.floor_sensor().is_some() {
                            elevator.door_light(true);
                            // Note: Blocking sleep, consider async/timer if this becomes an issue
                            std::thread::sleep(Duration::from_secs(3));
                            elevator.door_light(false);
                        }

                        // Persist the cleared state (optional but good practice)
                        fault_handler::persist_elevator_state(
                            &local_id,
                            elevator.current_floor,
                            elevator.current_direction,
                            &elevator.call_buttons
                        ).unwrap_or_else(|e| eprintln!("Failed to persist state after stop: {}", e));

                    }

                    // Reassign the collected hall calls outside the elevator lock
                    for (floor, direction, timestamp) in calls_to_reassign {
                         println!("Stop button: Reassigning hall call: floor {}, direction {}", floor, direction);
                         // Trigger reassignment by the system
                         elevator_system.assign_hall_call(floor, direction, timestamp);
                    }


                    // Broadcast updated (stopped) state
                    elevator_system.broadcast_state();
                }
            },
            
            // Handle obstruction
            recv(obstruction_rx) -> obstruction => {
                let obstr = obstruction.unwrap();
                let mut elevator = elevator_system.local_elevator.lock().unwrap();
                
                // Only consider obstruction when at a floor
                if elevator.floor_sensor().is_some() {
                    let current_time = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                        Ok(n) => n.as_secs(),
                        Err(_) => 0,
                    };
                    
                    // Update obstruction state
                    elevator.is_obstructed = obstr;
                    
                    if obstr {
                        println!("Obstruction detected at floor {}", elevator.current_floor);
                        
                        // Start tracking obstruction time
                        if elevator.obstruction_start_time.is_none() {
                            elevator.obstruction_start_time = Some(current_time);
                            println!("Started obstruction timer at {}", current_time);
                        }
                        
                        // Force the door to stay open
                        elevator.door_light(true);
                        
                        // Reassign calls without waiting 10 seconds
                        drop(elevator); 
                        
                        // Find and reassign this elevator's calls
                        let calls_to_reassign = {
                            let mut calls = Vec::new();
                            let hall_calls = elevator_system.hall_calls.lock().unwrap();
                            
                            for ((floor, direction), (assigned_to, timestamp)) in hall_calls.iter() {
                                if assigned_to == &elevator_system.local_id {
                                    calls.push((*floor, *direction, *timestamp));
                                    println!("Immediately reassigning call (floor {}, dir {}) due to obstruction", 
                                            floor, direction_to_string(*direction));
                                }
                            }
                            calls
                        };
                        
                        // Reassign each call
                        for (floor, direction, timestamp) in calls_to_reassign {
                            // Mark the call as unassigned
                            {
                                let mut hall_calls = elevator_system.hall_calls.lock().unwrap();
                                hall_calls.insert((floor, direction), (String::new(), timestamp));
                            }
                            
                            // Reassign the call
                            elevator_system.assign_hall_call(floor, direction, timestamp);
                        }
                        
                        // Re-acquire elevator lock
                        elevator = elevator_system.local_elevator.lock().unwrap();
                    } else {
                        // Obstruction cleared
                        println!("Obstruction cleared at floor {}", elevator.current_floor);
                        
                        // Reset obstruction timer
                        elevator.obstruction_start_time = None;
                        
                    }
                } else {
                    // Not at a floor, simply update state
                    elevator.is_obstructed = obstr;
                    println!("Obstruction state updated: {}", obstr);
                }
                
                // Broadcast updated state to peers
                drop(elevator);
                elevator_system.broadcast_state();
            },
        }
    }
}