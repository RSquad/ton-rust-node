/// A lock-free Single-Producer-Single-Consumer (SPSC) FIFO channel.
pub mod spsc;

/// A lock-free Multi-Producer-Single-Consumer (MPSC) FIFO channel.
pub mod mpsc;

/// A lock-free Single-Producer-Multi-Consumer (SPMC) FIFO channel.
pub mod spmc;

/// A lock-free Multi-Producer-Multi-Consumer (MPMC) FIFO channel.
pub mod mpmc;

/// The error of `Sender::send` operation. Occurs if all receivers were
/// disconnected.
#[derive(Debug, Clone, Copy)]
pub struct NoRecv<T> {
    /// The message which was attempted to be sent.
    pub message: T,
}

/// The error of `Receiver::recv` operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvErr {
    /// Returned when there are no messages, the channel is empty, but there
    /// are still senders connected.
    NoMessage,
    /// Returned when all senders were disconnected.
    NoSender,
}
