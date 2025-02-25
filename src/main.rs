/* Keyboard controls for the simulator:

    Hall call buttons for single elevator:
     - Up-buttons: keys 'q, w, e' = floor '1, 2, 3'
     - Down-buttons: keys 's, d, f' = floor '0, 1, 2'
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

fn main() -> std::io::Result<()> {

    let elev_num_floors = 4; // Total floor count
    
    // Initialize the elevator connection to the server adress.
    // The elevator struct in elev.rs creates a mutex lock for the TCP stream
    let elevator = e::Elevator::init("localhost:15657", elev_num_floors)?;

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
    // Define variable 'dirn' to keep track of current direction; down, up or stop.
    let mut dirn = e::DIRN_DOWN;

    // If the elevator isn't on a specific floor when we start, move down until it reaches one.
    // if elevator.floor_sensor().is_none() {
    //     elevator.motor_direction(dirn);
    // }
    let (arrived_floor_tx, arrived_floor_rx) = cbc::unbounded::<u8>();
    let mut starting_floor = floor_sensor_rx.recv().unwrap();
    while starting_floor != 0 {
        elevator.motor_direction(e::DIRN_DOWN);
        starting_floor = floor_sensor_rx.recv().unwrap();
    }
    // Main loop that uses 'select!' to wait for messages from any of the channels:
    loop {
        cbc::select! {
            // If we receive that a call button is pressed from the thread:
            recv(call_button_rx) -> button_type => {
                let call_button = button_type.unwrap();
                println!("{:#?}", call_button);
                //  let floor = floor_sensor_rx.recv().unwrap();
                // println!("{:#?}",floor);
                // Turn on the corresponding call button light
                elevator.call_button_light(call_button.floor, call_button.call, true);
                let go_floor = call_button.floor;
                let floor = floor_sensor_rx.recv().unwrap();
                if floor < go_floor {
                    dirn = e::DIRN_UP;
                } else if floor > go_floor {
                    dirn = e::DIRN_DOWN;
                } else {
                    dirn = e::DIRN_STOP;
                }
                elevator.motor_direction(dirn);
                arrived_floor_tx.send(floor).unwrap();


            }

            // If we receive that a new floor is reached from the thread:
            recv(floor_sensor_rx) -> a => {
                let floor = a.unwrap();
                println!("Floor: {:#?}", floor);
                elevator.floor_indicator(floor);// Update the floor indicator when a new floor is reached
                dirn =
                    if floor == 0 { e::DIRN_STOP}
                    else { dirn };
                elevator.motor_direction(dirn);
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
            recv(arrived_floor_rx) -> a => {
                let arrived_floor = a.unwrap();
                if(floor_sensor_rx.recv().unwrap() == arrived_floor) {
                    elevator.motor_direction(e::DIRN_STOP);
                }

            }
        }
    }
}
