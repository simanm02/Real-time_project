# Elevator Control System Rust Docs

## Project Overview
This code implements a control system for one signel elevator operating across four floors. The system is built in Rust and communicates with the elevator hardware through a TCP connection.

Call queues and prioritization are handled according to the spec. The obstruction lever is implemented in such a way that it only halts the elevator if the door is open, or, if it's currently moving towards a floor, it waits and halts the elevator at the moment when the door opens. The stop button stops the elevator immediately no matter what, and clears all queues.

## System structure
The system spawns individual crossbeam channels for communication between threads that can't be accessed by any other process. These are threads that send events to the main control loop, and each sensor type has its own dedicated polling thread, including call buttons, floor sensors, stop button and obstruction sensor.

The elevator hardware then uses TCP in order for the elevator hardware to communicate with the polling threads, which has channels to the main control loop:

[Elevator Hardware] <--TCP--> [Polling Threads] <--Channels--> [Main Control Loop]
