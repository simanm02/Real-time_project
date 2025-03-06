use std::thread::*;
use std::time::*;

use crossbeam_channel as cbc;

use driver_rust::elevio;
use driver_rust::elevio::elev as e;
use driver_rust::elevio::elev::Elevator;

// Decide direction based on call received
fn direction_call (go_floor: u8, floor: u8) -> u8 {
    let dirn: u8;

    if floor < go_floor {
        dirn = e::DIRN_UP;
    } else if floor > go_floor {
        dirn = e::DIRN_DOWN;
        println!("Going down");
    } else {
        dirn = e::DIRN_STOP;
    }

    println!("Direction: {:#?}", dirn);
    dirn
}

fn add_call_request (elevator: &mut Elevator, go_floor: u8, go_call: u8) {
    let callbutton = vec![go_floor, go_call];
    if !elevator.call_buttons.iter().any(|x| x == &callbutton) {
        elevator.call_buttons.push(callbutton);
    }
}

fn start_elevator (elevator: &mut Elevator, go_floor: u8,floor: u8,  mut dirn: u8) {
    println!("Direction2: {:#?}", dirn);
    if elevator.call_buttons.len() == 1 || (elevator.call_buttons.len() > 1 && elevator.current_direction == e::DIRN_STOP) {
        dirn = direction_call(go_floor, floor);
        elevator.current_direction = dirn;
        elevator.motor_direction(dirn);
    }
}

// Function to handle calls when the elevator reaches a certain floor:
fn serve_call (elevator: &mut Elevator, floor: u8) {
    // Get the call type, either UP, DOWN or CAB, of the first call in the queue.
    let serving_call = elevator.call_buttons.get(0).unwrap().get(1).unwrap();
    
    // Check if there is a call for the current floor that matches either:
    // 1. The first call in the queue, or
    // 2. A cab call (call from inside the elevator) for this floor
    if let Some(pos) = elevator.call_buttons.iter().position(|call|
        call[0] == floor && (call[1] == *serving_call || call[1] == e::CAB))  {
        
        // Stop elevator at desired floor
        elevator.motor_direction(e::DIRN_STOP);
        
        // Turn off call button light for served floor and call button
        elevator.call_button_light(floor,*serving_call, false);
        
        // Remove call from queue
        elevator.call_buttons.remove(pos);
        println!("Call for floor {} removed", floor);

        // Open door (turn on door light) for 3 seconds
        elevator.door_light(true);
        std::thread::sleep(Duration::from_secs(3));
        elevator.door_light(false);

        // Sort call queue to prioritize cab calls
        elevator.call_buttons.sort_by_key(|call| if call[1] == e::CAB { 0 } else { 1 });
        
        // Decide next direction based on remaining calls
        if let Some(next_call) = elevator.call_buttons.first() {
            let next_floor = next_call[0];

            // Calculate direction to next call
            let mut new_dir = if next_floor > floor {
                e::DIRN_UP
            } else if next_floor < floor {
                e::DIRN_DOWN
            } else {
                // If next call is for the current floor, serve it immediately
                elevator.call_button_light(floor, next_call[1], false);
                elevator.call_buttons.remove(0);
                e::DIRN_STOP

            };

            // If we've cleared all calls for the current floor, check if there are more calls
            if let Some(next_call) = elevator.call_buttons.first() {
                if new_dir == e::DIRN_STOP {
                    new_dir = direction_call(next_call[0], floor);
                }
            }

            // Update elevator direction and motor
            elevator.current_direction = new_dir;
            elevator.motor_direction(new_dir);
        }
    }
}

fn main() -> std::io::Result<()> {

    let elev_num_floors = 4; // Total floor count
    
    // Initialize the elevator connection to the server adress.
    // The elevator struct in elev.rs creates a mutex lock for the TCP stream
    let mut elevator = e::Elevator::init("localhost:15657", elev_num_floors)?;

    println!("Elevator started:\n{:#?}", elevator);

    // Polling period which reads sensor data at 25 ms intervals.
    let poll_period = Duration::from_millis(25);

    // Creates a crossbream channel, so that the call buttons.
    // can receive and transmitt messages via TCP.
    let (call_button_tx, call_button_rx) = cbc::unbounded::<elevio::poll::CallButton>();
    {
        // Clone the elevator handle so that there can be a new thread dedicated to it.
        let elevator = elevator.clone();

        // "Spawn" a new dedicated thread that continuously polls
        // call button presses and sends them (when pressed) to call_button_tx
        spawn(move || elevio::poll::call_buttons(elevator, call_button_tx, poll_period));
    }

    // Create crossbeam channel for the floor sensors when elevator passes.
    let (floor_sensor_tx, floor_sensor_rx) = cbc::unbounded::<u8>();
    {
        // Again clones and creates another dedicated thread to poll floor sensor data.
        let elevator_new = elevator.clone();
        spawn(move || elevio::poll::floor_sensor(elevator_new, floor_sensor_tx, poll_period));
    }

    // Create crossbeam channel for stoppbuttons
    let (stop_button_tx, stop_button_rx) = cbc::unbounded::<bool>();
    {
        // Clone and create dedicated thread to poll stop button data.
        let elevator = elevator.clone();
        spawn(move || elevio::poll::stop_button(elevator, stop_button_tx, poll_period));
    }

    // Create crossbeam channel for obstruction lever
    let (obstruction_tx, obstruction_rx) = cbc::unbounded::<bool>();
    {
        // Clone and create dedicated thread to poll pull lever data
        let elevator = elevator.clone();
        spawn(move || elevio::poll::obstruction(elevator, obstruction_tx, poll_period));
    }
    // let (buttonarray_tx, buttonarray_rx) = cbc::unbounded::<Vec<u8>>();

    // Define variable 'dirn' to keep track of current direction; down, up or stop.
    let dirn = e::DIRN_STOP;

    //If the elevator isn't on a specific floor when we start, move down until it reaches one.
    if elevator.floor_sensor().is_none() {
        elevator.motor_direction(e::DIRN_DOWN);
    }

    // Turn off all call button lights at startup
    for call_type in 0..3 {
        for floor in 0..3 {
            elevator.call_button_light(floor, call_type, false);
        }
    }

    // Move elevator to ground floor at startup
    let mut starting_floor = floor_sensor_rx.recv().unwrap();
    while starting_floor != 0 {
        elevator.motor_direction(e::DIRN_DOWN);
        starting_floor = floor_sensor_rx.recv().unwrap();
        elevator.floor_indicator(starting_floor);
    }
    elevator.motor_direction(e::DIRN_STOP);

    elevator.floor_indicator(0);

    // Main loop that uses 'select!' to wait for messages from any of the channels:
    loop {
        cbc::select! {
            // If we receive that a call button is pressed from the thread:
            recv(call_button_rx) -> button_type => {
                let call_button = button_type.unwrap();
                println!("{:#?}", call_button);

                // Turn on the corresponding call button light
                elevator.call_button_light(call_button.floor, call_button.call, true);

                // Extract data and assign
                let go_floor = call_button.floor;
                let go_call = call_button.call;
                let floor = elevator.current_floor;

                // Add to queue and start elevator if needed
                add_call_request(&mut elevator, go_floor, go_call);
                start_elevator(&mut elevator,go_floor,floor, dirn);
            },

            // If we receive that a new floor is reached from the thread:
            recv(floor_sensor_rx) -> floor_sensor_data => {
                let floor = floor_sensor_data.unwrap();
                elevator.current_floor = floor;
                println!("Floor: {:#?}", floor);
                elevator.floor_indicator(floor);// Update the floor indicator when a new floor is reached
                serve_call(&mut elevator, floor);
            },

            // If we receive that a stop button is pressed from the thread:
            recv(stop_button_rx) -> stop_btn => {
                let stop = stop_btn.unwrap();
                println!("Stop button: {:?}", stop);
                if stop {
                    // Immediately stop the elevator
                    elevator.motor_direction(e::DIRN_STOP);
            
                    // Turn off all call button lights
                    for f in 0..elev_num_floors {
                        for c in 0..3 {
                            elevator.call_button_light(f, c, false);
                        }
                    }
                    // Clear pending call requests
                    elevator.call_buttons.clear();
            
                    // Optionally, open the door to simulate an emergency stop procedure
                    elevator.door_light(true);
                    std::thread::sleep(Duration::from_secs(3));
                    elevator.door_light(false);
            
                    // Continue to restart the loop
                    continue;
                }
            },
            // If we receive that there is an obstruction:
            recv(obstruction_rx) -> obstruction => {
                let mut obstr = obstruction.unwrap();
                if !elevator.floor_sensor().is_none() && obstr {
                    println!("Obstruction: {:#?}", obstr);
                    while obstr {
                        elevator.motor_direction(e::DIRN_STOP);
                        obstr = obstruction_rx.recv().unwrap();
                    }
                } else {
                    obstr = false;
                    println!("Obstruction: {:#?}", obstr);
                }
            },
        }
    }
}
