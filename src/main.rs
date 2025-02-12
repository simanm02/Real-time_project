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
use driver_rust::elevio::poll::CallButton;

fn hall_call_start_dir (go_floor: u8, floor: u8, mut dirn: u8) -> u8 {
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
/*
fn stop_elevator_at_floor_and_start (elevator: &mut Elevator, floor: u8, mut dirn: u8) {
    let call = elevator.call_buttons.get(0).unwrap().get(1).unwrap();
    let mut iter = elevator.call_buttons.iter();
    let call_button_index = iter.position(|x| *x.get(0).unwrap() == floor); // little bit of magic to find the index of the floor we are going to
    if call_button_index.is_some() {
        let go_floor = elevator.call_buttons.get(call_button_index.unwrap()).unwrap().get(0).unwrap(); // get the floor we are at(bit java style sry
        let go_call = elevator.call_buttons.get(call_button_index.unwrap()).unwrap().get(1).unwrap();
        println!("Go floor: {:#?}", go_floor);
        dirn = elevator.current_direction;
        dirn = hall_call_stop(*go_floor, floor,*go_call,*call, dirn);
        if(dirn == e::DIRN_STOP){
            elevator.call_buttons.remove(call_button_index.unwrap());
        }
        elevator.motor_direction(dirn);
        sleep(Duration::from_millis(1000)); // wait in the floor for a second(door open..)
        if elevator.call_buttons.len() != 0 {
            let go_floor = elevator.call_buttons.get(0).unwrap().get(0).unwrap();
            dirn = hall_call_start_dir(*go_floor, floor, dirn);
            elevator.current_direction = dirn;
            println!("Direction: {:#?}", dirn);
            elevator.motor_direction(dirn);
        }
    }
} */

fn stop_elevator_at_floor_and_start(elevator: &mut Elevator, floor: u8) {
    if let Some(pos) = elevator.call_buttons.iter().position(|call| call[0] == floor) {
        // Stop elevator
        elevator.motor_direction(e::DIRN_STOP);

        // Disable call button lights
        for call_type in 0..3 {
            elevator.call_button_light(floor, call_type, false);
        }

        // Remove the call from the list
        elevator.call_buttons.remove(pos);
        println!("Call for floor {} removed", floor);

        elevator.door_light(true);
        std::thread::sleep(Duration::from_secs(1));
        elevator.door_light(false);

        // Handle pending calls and decide next direction
        if let Some(next_call) = elevator.call_buttons.first() {
            let next_floor = next_call[0];
            let new_dir = if next_floor > floor {
                e::DIRN_UP
            } else if next_floor < floor {
                e::DIRN_DOWN
            } else {
                e::DIRN_STOP
            };
            elevator.current_direction;
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
    let (buttonarray_tx, buttonarray_rx) = cbc::unbounded::<Vec<u8>>();

    // Define variable 'dirn' to keep track of current direction; down, up or stop.
    let mut dirn = e::DIRN_STOP;

    //If the elevator isn't on a specific floor when we start, move down until it reaches one.
    if elevator.floor_sensor().is_none() {
        elevator.motor_direction(e::DIRN_DOWN);
    }

    let mut starting_floor = floor_sensor_rx.recv().unwrap();
    while starting_floor != 0 {
        elevator.motor_direction(e::DIRN_DOWN);
        starting_floor = floor_sensor_rx.recv().unwrap();
    }
    elevator.motor_direction(e::DIRN_STOP);

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
                //  let callbutton = vec![go_floor, go_call];
                // if !elevator.call_button.iter().any(|x| x == &callbutton) {
                //     elevator.call_button.push(callbutton);
                // }
                // if elevator.call_button.len() == 1 {
                //     dirn = hall_call_start_dir(go_floor, floor, dirn);
                //     elevator.motor_direction(dirn);
                // }
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
                println!("Stop button: {:#?}", stop);
                for f in 0..elev_num_floors {
                    for c in 0..3 {
                        elevator.call_button_light(f, c, false);
                    }
                }
                if stop {
                    elevator.motor_direction(e::DIRN_STOP);
                }
            },
            // If we receive that there is an obstruction:
            recv(obstruction_rx) -> a => {
                let obstr = a.unwrap();
                println!("Obstruction: {:#?}", obstr);
                elevator.motor_direction(if obstr { e::DIRN_STOP } else { dirn });
            },
        }
    }
}
