use std::time::Duration;
use std::thread::sleep;

use crate::elevio::elev as e;
use crate::elevio::elev::Elevator;
use crate::elevio::fault_handler;
use crate::elevio::system::ElevatorSystem;


pub fn direction_to_string(direction: u8) -> &'static str {
    match direction {
        e::DIRN_UP => "UP",
        e::DIRN_DOWN => "DOWN",
        e::DIRN_STOP => "STOP",
        _ => "UNKNOWN"
    }
}

// Function to start elevator movement based on its call queue
pub fn start_elevator(elevator: &mut Elevator, go_floor: u8,  mut dirn: u8) {
    if elevator.call_buttons.len() == 1 || (elevator.call_buttons.len() > 1 && elevator.current_direction == e::DIRN_STOP) {
        dirn = hall_call_start_dir(go_floor, elevator.current_floor, dirn);
        elevator.current_direction = dirn;
        elevator.motor_direction(dirn);
    }
}
pub fn decide_next_call(elevator: &mut Elevator) -> Option<&Vec<u8>> {
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
pub fn hall_call_start_dir (go_floor: u8, floor: u8, mut dirn: u8) -> u8 {
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

pub fn decide_direction_by_call(call: u8) -> u8 {
    if call == 0 {
        e::DIRN_UP
    } else if call == 1 {
        e::DIRN_DOWN
    } else {
        e::DIRN_STOP
    }
}
pub fn decide_call_by_direction(dirn: u8) -> u8 {
    if dirn == e::DIRN_UP {
        0
    } else if dirn == e::DIRN_DOWN {
        1
    } else {
        2
    }
}

pub fn is_more_request_in_dir(elevator:  Elevator) -> bool {
    elevator.call_buttons.iter().any(|call| {
        (elevator.current_direction == e::DIRN_UP && call[0] > elevator.current_floor) ||
            (elevator.current_direction == e::DIRN_DOWN && call[0] < elevator.current_floor)
    })
}

pub fn opposite_dir(dirn: u8) -> u8 {
    if dirn == e::DIRN_UP {
        e::DIRN_DOWN
    } else {
        e::DIRN_UP
    }
}

pub fn find_call_button_index(call_button: Vec<u8>,elevator: &mut Elevator) -> Option<usize> {
    elevator.call_buttons.iter().position(|call| call == &call_button)
}

pub fn serve_call(elevator_system: &ElevatorSystem, floor: u8) {
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

        println!("Persisting state after removing extra cab call at floor {}", floor);
         fault_handler::persist_elevator_state(
             &elevator_system.local_id,
             elevator.current_floor,
             elevator.current_direction,
             &elevator.call_buttons
         ).unwrap_or_else(|e| eprintln!("Failed to persist state: {}", e));
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
    } else {
        println!("Persisting state after serving cab call at floor {}", floor);
        fault_handler::persist_elevator_state(
            &elevator_system.local_id,
            elevator.current_floor,
            elevator.current_direction,
            &elevator.call_buttons
        ).unwrap_or_else(|e| eprintln!("Failed to persist state: {}", e));
    }

    println!("Call for floor {} removed", floor);

    // Open door and handle obstruction properly
    elevator.door_light(true);

    // Check for obstruction before starting the wait
    if elevator.is_obstructed {
        println!("Door kept open due to active obstruction");
        // Release the elevator lock and return it to main loop
        return;
    }

    // Release the lock while waiting to avoid blocking other operations
    drop(elevator);

    // Wait for door delay
    let start_time = std::time::Instant::now();
    let door_delay = Duration::from_secs(3);

    // Check periodically for obstruction during the waiting period
    while start_time.elapsed() < door_delay {
        std::thread::sleep(Duration::from_millis(100)); // Check every 100ms
        
        // Check if obstruction is active
        let is_blocked = {
            let e = elevator_system.local_elevator.lock().unwrap();
            e.is_obstructed
        };
        
        if is_blocked {
            println!("Door operation interrupted by obstruction");
            return; // Goes to main loop
        }
    }

    // Reacquire the lock after waiting
    elevator = elevator_system.local_elevator.lock().unwrap();

    // Decide next call.
    if let Some(next_call) = decide_next_call(&mut *elevator) {
        let next_floor = next_call[0];
        let next_call_type = next_call[1];
        let mut new_dir = if next_floor > floor {
            e::DIRN_UP
        } else if next_floor < floor {
            e::DIRN_DOWN
        } else {
            let is_immediate_cab_call = next_call_type == e::CAB;

            let next_call_index = find_call_button_index(next_call.clone(), &mut *elevator).unwrap();



            if !is_immediate_cab_call {
                drop(elevator);
                elevator_system.complete_call(floor, next_call_type);
                elevator = elevator_system.local_elevator.lock().unwrap();
            }
            elevator.call_button_light(floor, next_call_type, false);
            elevator.call_buttons.remove(next_call_index);

            if is_immediate_cab_call {
                println!("Persisting state after serving immediate cab call at floor {}", floor);
                fault_handler::persist_elevator_state(
                    &elevator_system.local_id,
                    elevator.current_floor,
                    elevator.current_direction,
                    &elevator.call_buttons
                ).unwrap_or_else(|e| eprintln!("Failed to persist state: {}", e));
            }

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