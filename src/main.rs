/* Keyboard controls for the simulator:

    Hall call buttons for single elevator:
     - Up-buttons: keys 'q, w, e' = floor '1, 2, 3'
     - Down-buttons: keys 's, d, f' = floor '1, 2, 3'
     - Cab call buttons: keys 'z, x, c, v' = floor '0, 1, 2, 3'

    Stop-button: key 'p'
    Obstruction lever: key '-'
    
    Manual button override:
     - Down: '7'
     - Stop: '8'
     - Up: '9'
     - Move back in bounds: '0'

*/

use std::thread::*;
use std::time::*;

use crossbeam_channel as cbc;

use driver_rust::elevio;
use driver_rust::elevio::elev as e;
use driver_rust::elevio::elev::Elevator;
use driver_rust::elevio::poll::{call_buttons, CallButton};

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
fn hall_call_stop (go_floor: u8, floor: u8,go_call: u8 ,call: u8,mut dirn: u8)-> u8 {
    if floor == go_floor && go_call == call {
        dirn = e::DIRN_STOP;
        println!("Stopping at floor: {:#?}", floor);
    }
    dirn
}
fn add_call_request_to_elevator (elevator: &mut Elevator, go_floor: u8, go_call: u8) {
    let callbutton = vec![go_floor, go_call];
    if !elevator.call_buttons.iter().any(|x| x == &callbutton) {
        elevator.call_buttons.push(callbutton);
    }


}
fn start_elevator (elevator: &mut Elevator, go_floor: u8,floor: u8,  mut dirn: u8) {
    println!("Direction2: {:#?}", dirn);
    if elevator.call_buttons.len() == 1 || (elevator.call_buttons.len() > 1 && elevator.current_direction == e::DIRN_STOP) {
        dirn = hall_call_start_dir(go_floor, floor, dirn);
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
fn stop_elevator_at_floor_and_start(elevator: &mut Elevator, floor: u8) {
    if let Some(pos) = elevator.call_buttons.iter().position(|call| call[0] == floor &&
        (decide_direction_by_call(call[1]) == elevator.current_direction || call[1] == e::CAB)
        || !is_more_request_in_dir(elevator.clone())) {

        // Stop elevator
        elevator.motor_direction(e::DIRN_STOP);

        //find the serving call
        let mut serving_call;
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

        println!("Call for floor {} removed", floor);
        elevator.door_light(true);
        std::thread::sleep(Duration::from_secs(3));
        //elevator.door_light(false);

        // Handle pending calls and decide next direction ;
        if let Some(next_call) = decide_next_call(elevator) {

            let next_floor = next_call[0];
            let next_call_type = next_call[1];
            let mut new_dir = if next_floor > floor {
                e::DIRN_UP
            } else if next_floor < floor {
                e::DIRN_DOWN
            } else {
                let next_call_index = find_call_button_index(next_call.clone(),elevator).unwrap();
                elevator.call_button_light(floor, next_call_type, false);
                elevator.call_buttons.remove(next_call_index);
                e::DIRN_STOP

            };
            if let Some(next_call) = decide_next_call(elevator) {
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
    let (buttonarray_tx, buttonarray_rx) = cbc::unbounded::<Vec<u8>>();

    // Define variable 'dirn' to keep track of current direction; down, up or stop.
    let mut dirn = e::DIRN_STOP;

    //If the elevator isn't on a specific floor when we start, move down until it reaches one.
    if elevator.floor_sensor().is_none() {
        elevator.motor_direction(e::DIRN_DOWN);
    }

    for call_type in 0..3 {
        for floor in 0..3 {
            elevator.call_button_light(floor, call_type, false);
        }
    }

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
                let go_floor = call_button.floor;
                let go_call = call_button.call;
                let floor = elevator.current_floor;
                add_call_request_to_elevator(&mut elevator, go_floor, go_call);
                start_elevator(&mut elevator,go_floor,floor, dirn);

            }

            // If we receive that a new floor is reached from the thread:
            recv(floor_sensor_rx) -> a => {
                let floor = a.unwrap();
                elevator.current_floor = floor;
                println!("Floor: {:#?}", floor);
                elevator.floor_indicator(floor);// Update the floor indicator when a new floor is reached
                stop_elevator_at_floor_and_start(&mut elevator, floor);

            },

            // If we receive that a stop button is pressed from the thread:
            recv(stop_button_rx) -> a => {
                let stop = a.unwrap();
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
            recv(obstruction_rx) -> a => {
                let mut obstr = a.unwrap();
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
