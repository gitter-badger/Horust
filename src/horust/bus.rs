use crossbeam::channel::{unbounded, Receiver, Sender};
use std::fmt::Debug;

/// A simple bus implementation: distributes the messages among the queues
/// There is one single input pipe (`public_sender` ; `receiver`). The sender side is shared among
/// all the publishers. The bus reads from the receiver, and publishes to all the `senders`.
#[derive(Debug)]
pub struct Bus<T>
where
    T: Clone + Debug,
{
    /// Bus input - sender side
    shared_sender: Sender<Message<T>>,
    /// Bus input - receiver side
    receiver: Receiver<Message<T>>,
    /// Bus output - all the senders
    senders: Vec<(u64, Sender<Message<T>>)>,
    /// Forward the message to the sender as well.
    forward_to_sender: bool,
}

impl<T> Bus<T>
where
    T: Clone + Debug,
{
    pub fn new() -> Self {
        let (public_sender, receiver) = unbounded();
        Bus {
            shared_sender: public_sender,
            receiver,
            senders: Default::default(),
            forward_to_sender: true,
        }
    }

    /// Blocking
    pub fn run(self) {
        self.dispatch();
    }

    /// Add another connection to the bus
    pub fn join_bus(&mut self) -> BusConnector<T> {
        let (sender, receiver) = unbounded();
        self.senders.push((self.senders.len() as u64, sender));
        BusConnector::new(
            self.shared_sender.clone(),
            receiver,
            self.senders.len() as u64,
        )
    }

    /// Dispatching loop
    /// As soon as we don't have any senders it will exit
    fn dispatch(mut self) {
        drop(self.shared_sender);
        if self.forward_to_sender {
            for ev in self.receiver {
                self.senders
                    .retain(|(_idx, sender)| sender.send(ev.clone()).is_ok());
            }
        } else {
            for ev in self.receiver {
                self.senders.retain(|(idx, sender)| {
                    if *idx != ev.sender_id {
                        sender.send(ev.clone()).is_ok()
                    } else {
                        true
                    }
                });
            }
        }
    }
}

/// The payload with wrapped with some metadata
#[derive(Clone, Debug)]
struct Message<T>
where
    T: Clone + Debug,
{
    sender_id: u64,
    payload: T,
}
impl<T> Message<T>
where
    T: Clone + Debug,
{
    pub fn new(sender_id: u64, payload: T) -> Self {
        Self { payload, sender_id }
    }

    /// Consume the messages into the payload
    pub fn into_payload(self) -> T {
        self.payload
    }
}

/// A connector to the shared bus
#[derive(Debug, Clone)]
pub struct BusConnector<T>
where
    T: Clone + Debug,
{
    sender: Sender<Message<T>>,
    receiver: Receiver<Message<T>>,
    id: u64,
}
impl<T> BusConnector<T>
where
    T: Clone + Debug,
{
    fn new(sender: Sender<Message<T>>, receiver: Receiver<Message<T>>, id: u64) -> Self {
        Self {
            sender,
            receiver,
            id,
        }
    }
    fn wrap(&self, payload: T) -> Message<T> {
        Message::new(self.id, payload)
    }

    /// Blocking
    #[cfg(test)]
    pub fn get_n_events_blocking(&self, quantity: usize) -> Vec<T> {
        self.receiver
            .iter()
            .map(|m| m.into_payload())
            .take(quantity)
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = T> + '_ {
        self.receiver.iter().map(|message| message.into_payload())
    }

    /// Non blocking
    /// Todo: rename to be generic.
    pub fn try_get_events(&self) -> Vec<T> {
        self.receiver.try_iter().map(|m| m.into_payload()).collect()
    }

    pub(crate) fn send_event(&self, ev: T) {
        self.sender
            .send(self.wrap(ev))
            .expect("Failed sending update event!");
    }
}

#[cfg(test)]
mod test {

    use crate::horust::bus::{Bus, BusConnector};
    //TODO: remove this reference:
    use crate::horust::formats::{Event, ServiceStatus};
    use crossbeam::channel;
    use std::thread;
    use std::time::Duration;

    fn init_bus() -> (
        BusConnector<Event>,
        BusConnector<Event>,
        channel::Receiver<()>,
    ) {
        let mut bus = Bus::new();
        let a = bus.join_bus();
        let b = bus.join_bus();
        let (sender, receiver) = channel::bounded(48);

        let _handle = thread::spawn(move || {
            bus.run();
            sender
                .send(())
                .expect("test didn't terminate in time, so chan is closed!");
        });
        (a, b, receiver)
    }

    #[test]
    // tests get_events function both blocking and non-blocking
    fn test_get_events() {
        let (a, b, receiver_a) = init_bus();
        let ev = Event::new_status_changed(&"sample".to_string(), ServiceStatus::Initial);
        let (sender, receiver_b) = channel::bounded(48);

        let _handle = thread::spawn(move || {
            a.send_event(ev.clone());
            a.send_event(ev.clone());
            a.send_event(ev.clone());
            a.get_n_events_blocking(3);
            b.get_n_events_blocking(3);

            a.send_event(ev.clone());
            while a.receiver.is_empty() && b.receiver.is_empty() {
                thread::sleep(Duration::from_millis(200));
            }
            assert_eq!(a.try_get_events().len(), 1);
            assert_eq!(b.try_get_events().len(), 1);
            sender.send(()).unwrap();
        });

        receiver_a
            .recv_timeout(Duration::from_secs(3))
            .expect("Didn't receive an answer on time.");

        receiver_b
            .recv_timeout(Duration::from_secs(3))
            .expect("Didn't receive an answer on time.");
    }

    #[test]
    fn test_bus_simple() {
        let (a, b, receiver) = init_bus();
        let ev = Event::new_status_changed(&"sample".to_string(), ServiceStatus::Initial);
        a.send_event(ev.clone());
        assert_eq!(a.receiver.recv().unwrap().into_payload(), ev);
        assert_eq!(b.receiver.recv().unwrap().into_payload(), ev);
        drop(a);
        drop(b);
        receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("Didn't receive an answer on time.");
    }

    #[test]
    fn test_stress() {
        let mut bus = Bus::new();
        let mut connectors = vec![];
        let last = bus.join_bus();
        for _i in 0..100 {
            connectors.push(bus.join_bus());
        }
        let (sender, receiver) = channel::bounded(48);
        let _handle = thread::spawn(move || {
            bus.run();
            sender
                .send(())
                .expect("test didn't terminate in time, so chan is closed!");
        });
        let ev = Event::new_status_changed(&"sample".to_string(), ServiceStatus::Initial);
        let exit_ev = Event::ShuttingDownInitiated;

        for _i in 0..100 {
            last.send_event(ev.clone());
            for recv in &connectors {
                assert_eq!(recv.receiver.recv().unwrap().into_payload(), ev);
            }
            let to_stop = connectors.pop().unwrap();
            to_stop.send_event(exit_ev.clone());
            for recv in &connectors {
                assert_eq!(recv.receiver.recv().unwrap().into_payload(), exit_ev);
            }
        }
        last.send_event(Event::ShuttingDownInitiated);
        drop(connectors);
        drop(last);
        receiver
            .recv_timeout(Duration::from_secs(15))
            .expect("Didn't receive an answer on time.");
    }
}
