fn main() {
    println!("Hello, world!");
    let ctx = zmq::Context::new();
    let socket = ctx.socket(zmq::REQ).unwrap();
    socket.connect("tcp://127.0.0.1:1234").unwrap();
    let mut msg = zmq::Message::new();
    for request_nbr in 0..10 {
        println!("Sending Hello {}...", request_nbr);
        socket.send("Hello", 0).unwrap();

        socket.recv(&mut msg, 0).unwrap();
        println!("Received World {}: {}", msg.as_str().unwrap(), request_nbr);
    }
}
