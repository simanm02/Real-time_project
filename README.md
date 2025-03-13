# Elevator Control System Rust Docs

## Project Overview
This code implements a control system for one signel elevator operating across four floors. The system is built in Rust and communicates with the elevator hardware through a TCP connection.

Call queues and prioritization are handled according to the spec. The obstruction lever is implemented in such a way that it only halts the elevator if the door is open, or, if it's currently moving towards a floor, it waits and halts the elevator at the moment when the door opens. The stop button stops the elevator immediately no matter what, and clears all queues.

## System structure
The system spawns individual crossbeam channels for communication between threads that can't be accessed by any other process. These are threads that send events to the main control loop, and each sensor type has its own dedicated polling thread, including call buttons, floor sensors, stop button and obstruction sensor.

The elevator hardware then uses TCP in order for the elevator hardware to communicate with the polling threads, which has channels to the main control loop.

Elevator calls are stored in a vector within the Elevator struct, with cabin calls given priority over hall calls.

## Module overview

### src/lib.rs
This is the "library" file that connects the supporting files in the "elevio" folder, which is used in main.rs, and makes it clear which files are implemented and doing what.

### src/elevio/elev.rs
This file contains functions that manipulate the various elevator values and states such as lights, stop button true/false, obstruction true/false, motor direction, etc. All of this is implemented in the core `Elevator` struct that makes up all of its parameters.

### src/elevio/poll.rs
This file implements the polling threads containing the data used by main.rs to determine system behavior. The function `call_buttons` continuously checks for call button presses, `floor_sensor` continuously checks for which floor the elevator is at, and `stop_button` and `obstruction` also monitor whether or not someone has activated the stop button or obstruction lever respectively. 

### src/main.rs
This file handles the main logic of the system, including initialization, call queueing, deciding motor direction, looping through the polling of input data, etc. This behaviour is determined by polled inputs by the threads in poll.rs, which is passed on to main.rs where the system logic is handled by five main modules:
- `direction_call`: This function decides which direction the elevator should move in when a call is received.
- `add_call_request`: This creates a vector `callbutton`, containing data on floor and whether it is an UP, DOWN or CAB call. This is then pushed to the call_buttons queue.
- `start_elevator`: This is initialized once a call is received, and uses `direction_call` to move in the right direction.
- `serve_call`: This function contains the logic that decides which call it should move on to next once a call has been served. This can depend on where in the queue the call is, and which direction the elevator is originally intending to go next.
- `main`: This is the main function that combines all the functions above and sets variables on startup. It enters into the `loop`, which uses `cbc::select!`  to deteremine which functions to call and variables to set, based on exactly which thread it is receiving data from using `recv()`.

## Call Handling algorithm

When a call button is pressed:
1. The call is added to the queue if not already present
2. If the elevator is idle, it starts moving toward the call
3. When reaching a floor:
   - If the floor has a call matching current direction or cabin call, serve it
   - Open door for 3 seconds
   - Determine next direction based on remaining calls

Cabin calls take priority over hall calls to optimize efficiency.

## How to run and test
1. Have rust installed on the computer
2. Open a terminal and navigate to the cargo folder (the one containing 'Cargo.toml')
3. Build and run by typing 'cargo run'

## Future Improvements
- Implement multi-elevator support with distributed call allocation
- Add network resilience for handling disconnections
- Optimize call serving algorithm for better efficiency
