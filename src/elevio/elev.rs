#![allow(dead_code)]

use std::fmt; // For implementing Display/Debug traits
use std::io::*; // For read/write traits, etc.
use std::net::TcpStream; // For the TCP connection to the elevator server
use std::sync::*; // For Arc and Mutex

#[derive(Clone, Debug)]
// Elevator struct defines a mutex lock for the TCP stream and defines total floor count
pub struct Elevator {
    socket: Arc<Mutex<TcpStream>>,
    pub num_floors: u8,
}

// Constants for the elevator call buttons
pub const HALL_UP: u8 = 0;
pub const HALL_DOWN: u8 = 1;
pub const CAB: u8 = 2;

// Constants for the elevator direction
pub const DIRN_DOWN: u8 = u8::MAX;
pub const DIRN_STOP: u8 = 0;
pub const DIRN_UP: u8 = 1;

impl Elevator {
    // Initializes a new 'Elevator' by connecting a TCP socket to the server address
    pub fn init(addr: &str, num_floors: u8) -> Result<Elevator> {
        Ok(Self {
            // Arc + Mutex is used to create a thread-safe reference counted pointer
            socket: Arc::new(Mutex::new(TcpStream::connect(addr)?)),
            num_floors,
        })
    }

    // Function to set the motor direction
    pub fn motor_direction(&self, dirn: u8) {
        let buf = [1, dirn, 0, 0];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&buf).unwrap();
    }

    // Function to set the call button light
    pub fn call_button_light(&self, floor: u8, call: u8, on: bool) {
        let buf = [2, call, floor, on as u8];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&buf).unwrap();
    }

    // Function to set the floor indicator
    pub fn floor_indicator(&self, floor: u8) {
        let buf = [3, floor, 0, 0];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&buf).unwrap();
    }

    // Function to set the door light
    pub fn door_light(&self, on: bool) {
        let buf = [4, on as u8, 0, 0];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&buf).unwrap();
    }

    // Function to set the stop button light
    pub fn stop_button_light(&self, on: bool) {
        let buf = [5, on as u8, 0, 0];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&buf).unwrap();
    }

    // Function to check if a call button is pressed
    pub fn call_button(&self, floor: u8, call: u8) -> bool {
        let mut buf = [6, call, floor, 0];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&mut buf).unwrap();
        sock.read(&mut buf).unwrap();
        buf[1] != 0
    }

    // Function to check the floor sensor
    pub fn floor_sensor(&self) -> Option<u8> {
        let mut buf = [7, 0, 0, 0];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&buf).unwrap();
        sock.read(&mut buf).unwrap();
        if buf[1] != 0 {
            Some(buf[2])
        } else {
            None
        }
    }

    // Function to check if the stop button is pressed
    pub fn stop_button(&self) -> bool {
        let mut buf = [8, 0, 0, 0];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&buf).unwrap();
        sock.read(&mut buf).unwrap();
        buf[1] != 0
    }

    // Function to check if there is an obstruction
    pub fn obstruction(&self) -> bool {
        let mut buf = [9, 0, 0, 0];
        let mut sock = self.socket.lock().unwrap();
        sock.write(&buf).unwrap();
        sock.read(&mut buf).unwrap();
        buf[1] != 0
    }
}

// Implement Display trait for Elevator struct
impl fmt::Display for Elevator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let addr = self.socket.lock().unwrap().peer_addr().unwrap();
        write!(f, "Elevator@{}({})", addr, self.num_floors)
    }
}
