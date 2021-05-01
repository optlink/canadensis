#![no_std]

extern crate alloc;
extern crate fallible_collections;
extern crate hash32;
extern crate heapless;

extern crate canadensis_can;
extern crate canadensis_core;
extern crate canadensis_encoding;
extern crate canadensis_node;

mod hash;

// Reexports from other canadensis crates
pub use canadensis_can::*;
pub use canadensis_core::transfer;
pub use canadensis_core::*;
pub use canadensis_encoding::*;

pub mod node {
    //! Basic node functionality
    pub use canadensis_node::*;
}

use alloc::vec::Vec;
use core::iter;

use crate::hash::TrivialIndexMap;
use canadensis_core::time::Instant;
use canadensis_core::transfer::*;
use canadensis_encoding::{DeserializeError, Serialize, WriteCursor};
use fallible_collections::FallibleVec;

/// Payloads above this size (in bytes) will use a dynamically allocated buffer
const STACK_THRESHOLD: usize = 64;

/// Assembles transfers and manages transfer IDs to send messages
///
/// The subject ID is not part of this struct because it is used as a key in the map of publishers.
pub struct Publisher<I: Instant> {
    /// The ID of the next transfer sent
    next_transfer_id: TransferId,
    /// Timeout for sending a transfer, measured from the time the payload is serialized
    timeout: I::Duration,
    /// Priority for transfers
    priority: Priority,
    /// ID of this node
    source: NodeId,
}

impl<I: Instant> Publisher<I> {
    /// Creates a message transmitter
    ///
    /// node: The ID of this node
    ///
    /// priority: The priority to use for messages
    pub fn new(node_id: NodeId, timeout: I::Duration, priority: Priority) -> Self {
        Publisher {
            next_transfer_id: TransferId::const_default(),
            timeout,
            priority,
            source: node_id,
        }
    }

    pub fn publish<T>(
        &mut self,
        now: I,
        subject: SubjectId,
        payload: &T,
        transmitter: &mut Transmitter<I>,
    ) -> Result<(), OutOfMemoryError>
    where
        T: Serialize,
        I: Instant,
    {
        let deadline = self.timeout.clone() + now;
        // Part 1: Serialize
        do_serialize(payload, |payload_bytes| {
            // Part 2: Split into frames and put frames in the queue
            self.send_payload(subject, payload_bytes, deadline, transmitter)
        })
    }

    pub fn send_payload(
        &mut self,
        subject: SubjectId,
        payload: &[u8],
        deadline: I,
        transmitter: &mut Transmitter<I>,
    ) -> Result<(), OutOfMemoryError>
    where
        I: Clone,
    {
        // Assemble the transfer
        let transfer: Transfer<&[u8], I> = Transfer {
            timestamp: deadline,
            header: TransferHeader {
                source: self.source,
                priority: self.priority,
                kind: TransferKindHeader::Message(MessageHeader {
                    anonymous: false,
                    subject,
                }),
            },
            transfer_id: self.next_transfer_id,
            payload,
        };
        self.next_transfer_id = self.next_transfer_id.increment();

        transmitter.push(transfer)
    }
}

/// A transmitter that sends anonymous messages and does not require a node ID
pub struct AnonymousPublisher {
    /// The priority of transfers from this transmitter
    priority: Priority,
    /// The subject to transmit on
    subject: SubjectId,
    /// The ID of the next transfer sent
    next_transfer_id: TransferId,
}

impl AnonymousPublisher {
    /// Creates an anonymous message transmitter
    ///
    /// priority: The priority to use for messages
    ///
    /// subject: The subject ID to publish to
    pub fn new(priority: Priority, subject: SubjectId) -> Self {
        AnonymousPublisher {
            priority,
            subject,
            next_transfer_id: TransferId::const_default(),
        }
    }

    pub fn send<T, I>(
        &mut self,
        payload: &T,
        deadline: I,
        transmitter: &mut Transmitter<I>,
    ) -> Result<(), OutOfMemoryError>
    where
        T: Serialize,
        I: Clone,
    {
        // Part 1: Serialize
        do_serialize(payload, |payload_bytes| {
            self.send_payload(payload_bytes, deadline, transmitter)
        })
    }

    pub fn send_payload<I>(
        &mut self,
        payload: &[u8],
        deadline: I,
        transmitter: &mut Transmitter<I>,
    ) -> Result<(), OutOfMemoryError>
    where
        I: Clone,
    {
        // Assemble the transfer
        let transfer: Transfer<&[u8], I> = Transfer {
            timestamp: deadline,
            header: TransferHeader {
                source: make_pseudo_id(payload),
                priority: self.priority,
                kind: TransferKindHeader::Message(MessageHeader {
                    anonymous: false,
                    subject: self.subject,
                }),
            },
            transfer_id: self.next_transfer_id,
            payload,
        };
        self.next_transfer_id = self.next_transfer_id.increment();

        transmitter.push(transfer)
    }
}

/// Assembles transfers and manages transfer IDs to send service requests
pub struct Requester<I: Instant> {
    /// The ID of this node
    this_node: NodeId,
    /// The priority of transfers from this transmitter
    priority: Priority,
    /// The timeout for sending transfers
    timeout: I::Duration,
    /// The ID of the next transfer sent
    next_transfer_id: TransferId,
}

impl<I: Instant> Requester<I> {
    /// Creates a service request transmitter
    ///
    /// this_node: The ID of this node
    ///
    /// priority: The priority to use for messages
    ///
    /// service: The service ID to request
    pub fn new(this_node: NodeId, timeout: I::Duration, priority: Priority) -> Self {
        Requester {
            this_node,
            priority,
            timeout,
            next_transfer_id: TransferId::const_default(),
        }
    }

    pub fn send<T>(
        &mut self,
        now: I,
        service: ServiceId,
        payload: &T,
        destination: NodeId,
        transmitter: &mut Transmitter<I>,
    ) -> Result<(), OutOfMemoryError>
    where
        T: Serialize,
    {
        // Part 1: Serialize
        let deadline = self.timeout.clone() + now;
        do_serialize(payload, |payload_bytes| {
            // Part 2: Split into frames and send
            self.send_payload(payload_bytes, service, destination, deadline, transmitter)
        })
    }

    pub fn send_payload(
        &mut self,
        payload: &[u8],
        service: ServiceId,
        destination: NodeId,
        deadline: I,
        transmitter: &mut Transmitter<I>,
    ) -> Result<(), OutOfMemoryError> {
        // Assemble the transfer
        let transfer: Transfer<&[u8], I> = Transfer {
            timestamp: deadline,
            header: TransferHeader {
                source: self.this_node,
                priority: self.priority,
                kind: TransferKindHeader::Request(ServiceHeader {
                    service,
                    destination,
                }),
            },
            transfer_id: self.next_transfer_id,
            payload,
        };
        self.next_transfer_id = self.next_transfer_id.increment();

        transmitter.push(transfer)
    }
}

/// Serializes a payload into a buffer and passes the buffer to a closure
fn do_serialize<T, F>(payload: &T, operation: F) -> Result<(), OutOfMemoryError>
where
    T: Serialize,
    F: FnOnce(&[u8]) -> Result<(), OutOfMemoryError>,
{
    let payload_bytes = (payload.size_bits() + 7) / 8;
    if payload_bytes > STACK_THRESHOLD {
        let mut bytes: Vec<u8> = FallibleVec::try_with_capacity(payload_bytes)?;
        bytes.extend(iter::repeat(0).take(payload_bytes));
        payload.serialize(&mut WriteCursor::new(&mut bytes));
        operation(&bytes)
    } else {
        let mut bytes = [0u8; STACK_THRESHOLD];
        let bytes = &mut bytes[..payload_bytes];
        payload.serialize(&mut WriteCursor::new(bytes));
        operation(bytes)
    }
}

fn make_pseudo_id(payload: &[u8]) -> NodeId {
    // XOR some things. I don't know if this will actually work well.
    let mut id_bits = 37u8;
    for &byte in payload {
        id_bits ^= byte;
    }
    // Get a non-reserved ID
    loop {
        let id = NodeId::from_truncating(id_bits);
        if !id.is_diagnostic_reserved() {
            // Got a valid, non-diagnostic ID
            break id;
        }
        // This one is reserved. Try one lower.
        id_bits = id_bits.wrapping_sub(1);
    }
}

/// An incoming request to be processed
#[derive(Debug)]
struct RequestIn<T> {
    pub request: T,
    pub metadata: ResponseToken,
}

/// An outgoing response to a `RequestIn`, with the same metadata
#[derive(Debug)]
struct ResponseOut<T> {
    pub response: T,
    pub metadata: ResponseToken,
}

/// A token from a request that is needed to send a response
#[derive(Debug)]
pub struct ResponseToken {
    /// ID of the service that this is a response for
    service: ServiceId,
    /// ID of the node that sent the request
    client: NodeId,
    /// Transfer ID of the request transfer (and also the response transfer)
    transfer: TransferId,
    /// Priority of the request transfer (and also the response transfer)
    priority: Priority,
}

/// Something that may be able to handle incoming transfers
pub trait TransferHandler<C: Clock> {
    /// Potentially handles an incoming message transfer
    // TODO: Provide a way to react by publishing something?
    fn handle_message(&mut self, transfer: MessageTransfer<Vec<u8>, C::Instant>);

    /// Potentially handles an incoming service request
    fn handle_request(
        &mut self,
        transfer: ServiceTransfer<Vec<u8>, C::Instant>,
        token: ResponseToken,
        responder: Responder<'_, C>,
    );

    /// Potentially handles an incoming service response
    fn handle_response(&mut self, transfer: ServiceTransfer<Vec<u8>, C::Instant>);
}

/// A high-level interface with UAVCAN node functionality
///
/// Type parameters:
/// * `C`: The clock used to get the current time
/// * `H`: The `TransferHandler` that receives incoming transfers
/// * `P`: The maximum number of topics that can be published
/// * `R`: The maximum number of services for which requests can be sent
///
pub struct Node<C, H, const P: usize, const R: usize>
where
    C: Clock,
{
    clock: C,
    transmitter: Transmitter<C::Instant>,
    receiver: Receiver<C::Instant>,
    transfer_handler: H,
    node_id: NodeId,
    publishers: TrivialIndexMap<SubjectId, Publisher<C::Instant>, P>,
    // TODO: Need a separate next transfer ID for each destination node
    requesters: TrivialIndexMap<ServiceId, Requester<C::Instant>, R>,
}

impl<C, H, const P: usize, const R: usize> Node<C, H, P, R>
where
    C: Clock,
    H: TransferHandler<C>,
{
    pub fn new(clock: C, transfer_handler: H, node_id: NodeId, mtu: Mtu) -> Self {
        Node {
            clock,
            transmitter: Transmitter::new(mtu),
            receiver: Receiver::new(node_id),
            transfer_handler,
            node_id,
            publishers: TrivialIndexMap::new(),
            requesters: TrivialIndexMap::new(),
        }
    }

    pub fn accept_frame(&mut self, frame: Frame<C::Instant>) -> Result<(), OutOfMemoryError> {
        match self.receiver.accept(frame)? {
            Some(transfer) => {
                self.handle_incoming_transfer(transfer);
            }
            None => {}
        }
        Ok(())
    }

    fn handle_incoming_transfer(&mut self, transfer: Transfer<Vec<u8>, C::Instant>) {
        match transfer.header.kind {
            TransferKindHeader::Message(message_header) => {
                let message_transfer = MessageTransfer {
                    timestamp: transfer.timestamp,
                    header: MessageOnlyHeader {
                        source: transfer.header.source,
                        priority: transfer.header.priority,
                        message: message_header,
                    },
                    transfer_id: transfer.transfer_id,
                    payload: transfer.payload,
                };
                self.transfer_handler.handle_message(message_transfer);
            }
            TransferKindHeader::Request(service_header) => {
                let token = ResponseToken {
                    service: service_header.service,
                    client: transfer.header.source,
                    transfer: transfer.transfer_id,
                    priority: transfer.header.priority,
                };
                let service_transfer = ServiceTransfer {
                    timestamp: transfer.timestamp,
                    header: ServiceOnlyHeader {
                        source: transfer.header.source,
                        priority: transfer.header.priority,
                        service: service_header,
                    },
                    transfer_id: transfer.transfer_id,
                    payload: transfer.payload,
                };
                let responder = Responder {
                    this_node: self.node_id,
                    transmitter: &mut self.transmitter,
                    clock: &mut self.clock,
                };
                self.transfer_handler
                    .handle_request(service_transfer, token, responder);
            }
            TransferKindHeader::Response(service_header) => {
                let service_transfer = ServiceTransfer {
                    timestamp: transfer.timestamp,
                    header: ServiceOnlyHeader {
                        source: transfer.header.source,
                        priority: transfer.header.priority,
                        service: service_header,
                    },
                    transfer_id: transfer.transfer_id,
                    payload: transfer.payload,
                };
                self.transfer_handler.handle_response(service_transfer);
            }
        }
    }

    pub fn start_publishing_topic(
        &mut self,
        subject: SubjectId,
        timeout: <C::Instant as Instant>::Duration,
        priority: Priority,
    ) -> Result<SubscriptionToken, CapacityError> {
        let token = SubscriptionToken(subject.clone());
        self.publishers
            .insert(subject, Publisher::new(self.node_id, timeout, priority))
            .map(|_| token)
            .map_err(|_| CapacityError(()))
    }

    pub fn publish_to_topic<T>(
        &mut self,
        token: &SubscriptionToken,
        payload: &T,
    ) -> Result<(), OutOfMemoryError>
    where
        T: Serialize,
    {
        let publisher = self
            .publishers
            .get_mut(&token.0)
            .expect("Bug: Token exists but no subscriber");
        publisher.publish(self.clock.now(), token.0, payload, &mut self.transmitter)
    }

    /// Sets up to send requests for a service
    ///
    /// This also subscribes to the corresponding responses.
    pub fn start_sending_requests(
        &mut self,
        service: ServiceId,
        receive_timeout: <C::Instant as Instant>::Duration,
        response_payload_size_max: usize,
        priority: Priority,
    ) -> Result<ServiceToken, CapacityOrMemoryError> {
        let token = ServiceToken(service);
        self.requesters
            .insert(
                service,
                Requester::new(self.node_id, receive_timeout.clone(), priority),
            )
            .map_err(|_| CapacityError(()))?;
        match self
            .receiver
            .subscribe_response(service, response_payload_size_max, receive_timeout)
        {
            Ok(()) => Ok(token),
            Err(e) => {
                // Clean up requester
                self.requesters.remove(&service);
                Err(e.into())
            }
        }
    }

    pub fn send_request<T>(
        &mut self,
        token: &ServiceToken,
        payload: &T,
        destination: NodeId,
    ) -> Result<(), OutOfMemoryError>
    where
        T: Serialize,
    {
        let requester = self
            .requesters
            .get_mut(&token.0)
            .expect("Bug: No requester for token");
        requester.send(
            self.clock.now(),
            token.0,
            payload,
            destination,
            &mut self.transmitter,
        )
    }

    pub fn subscribe_message(
        &mut self,
        subject: SubjectId,
        payload_size_max: usize,
        timeout: <C::Instant as Instant>::Duration,
    ) -> Result<(), OutOfMemoryError> {
        self.receiver
            .subscribe_message(subject, payload_size_max, timeout)
    }

    pub fn subscribe_request(
        &mut self,
        service: ServiceId,
        payload_size_max: usize,
        timeout: <C::Instant as Instant>::Duration,
    ) -> Result<(), OutOfMemoryError> {
        self.receiver
            .subscribe_request(service, payload_size_max, timeout)
    }

    /// Returns a responder, which can be used to respond to service requests
    pub fn responder(&mut self) -> Responder<'_, C> {
        Responder {
            this_node: self.node_id,
            transmitter: &mut self.transmitter,
            clock: &mut self.clock,
        }
    }
}

pub struct Responder<'a, C>
where
    C: Clock,
{
    this_node: NodeId,
    transmitter: &'a mut Transmitter<C::Instant>,
    clock: &'a mut C,
}

impl<C> Responder<'_, C>
where
    C: Clock,
{
    pub fn send_response<T>(
        &mut self,
        token: ResponseToken,
        timeout: <C::Instant as Instant>::Duration,
        payload: &T,
    ) -> Result<(), OutOfMemoryError>
    where
        T: Serialize,
    {
        let now = self.clock.now();
        let deadline = timeout + now;
        do_serialize(payload, |payload| {
            self.send_response_payload(token, deadline, payload)
        })
    }

    fn send_response_payload(
        &mut self,
        token: ResponseToken,
        deadline: C::Instant,
        payload: &[u8],
    ) -> Result<(), OutOfMemoryError> {
        let transfer_out = Transfer {
            timestamp: deadline,
            header: TransferHeader {
                source: self.this_node,
                priority: token.priority,
                kind: TransferKindHeader::Response(ServiceHeader {
                    service: token.service,
                    destination: token.client,
                }),
            },
            transfer_id: token.transfer,
            payload,
        };
        self.transmitter.push(transfer_out)
    }
}

/// A token returned from start_publishing_topic that can be used to a publish a transfer using the
/// associated subject ID
pub struct SubscriptionToken(SubjectId);

/// A token returned from start_sending_requests that can be used to a request a service using the
/// associated service ID
pub struct ServiceToken(ServiceId);

/// An error indicating that an operation ran out of space in a fixed-capacity data structure
#[derive(Debug)]
pub struct CapacityError(());

#[derive(Debug)]
pub enum CapacityOrMemoryError {
    Capacity(CapacityError),
    OutOfMemory(OutOfMemoryError),
}

impl From<CapacityError> for CapacityOrMemoryError {
    fn from(inner: CapacityError) -> Self {
        CapacityOrMemoryError::Capacity(inner)
    }
}
impl From<OutOfMemoryError> for CapacityOrMemoryError {
    fn from(inner: OutOfMemoryError) -> Self {
        CapacityOrMemoryError::OutOfMemory(inner)
    }
}

pub trait Clock {
    type Instant: Instant;
    fn now(&mut self) -> Self::Instant;
}

/// Errors that may occur when responding to a request
#[derive(Debug)]
pub enum RespondError<E> {
    /// The request could not be deserialized
    Deserialize(DeserializeError),
    /// Memory was not available
    OutOfMemory(OutOfMemoryError),
    /// The request handler returned an error
    Handler(E),
}

impl<E> From<DeserializeError> for RespondError<E> {
    fn from(deserialize: DeserializeError) -> Self {
        RespondError::Deserialize(deserialize)
    }
}

impl<E> From<OutOfMemoryError> for RespondError<E> {
    fn from(oom: OutOfMemoryError) -> Self {
        RespondError::OutOfMemory(oom)
    }
}
