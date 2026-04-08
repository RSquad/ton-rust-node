/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::adnl::{
    common::{add_unbound_object_to_map, add_unbound_object_to_map_with_update, TARGET},
    node::AdnlNode,
};
use std::{
    collections::{HashMap, VecDeque},
    fmt::{Display, Formatter},
    io::{ErrorKind, Read},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc::{channel, Receiver, Sender, TryRecvError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use ton_block::{error, fail, Error, Result};

const MASK_TCP_ADDRESS: u32 = 0x40444E4C;
const SIZE_TCP_ADDRESS: usize = 10;
const SIZE_TCP_CONFIRM: usize = 1;
const SIZE_TCP_LENGTH: usize = 4;
const SIZE_UDP_BUFFER: usize = 1500;
const SOCKET_BUFFER_SIZE: usize = 1 << 24;
const SOCKET_TCP_BACKLOG: usize = 256;
const TIMEOUT_TCP_CONNECT_MS: Duration = Duration::from_millis(300);
const TIMEOUT_TCP_SPIN_MS: Duration = Duration::from_millis(10);

fn configure_tcp_socket(socket: &socket2::Socket, blocking: bool) -> Result<()> {
    // socket.set_send_buffer_size(1 << 26)?;
    socket.set_recv_buffer_size(SOCKET_BUFFER_SIZE)?;
    socket.set_tcp_nodelay(true)?;
    socket.set_nonblocking(!blocking)?;
    Ok(())
}

fn read_u16(b: &[u8]) -> Result<u16> {
    if b.len() < 2 {
        fail!("Not enough data ({}) to read u16", b.len());
    }
    Ok(((b[0] as u16) << 8) | (b[1] as u16))
}

fn read_u32(b: &[u8]) -> Result<u32> {
    if b.len() < 4 {
        fail!("Not enough data ({}) to read u32", b.len());
    }
    Ok(((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | (b[3] as u32))
}

fn sock_to_socket_addr(sock: &socket2::SockAddr) -> Result<SocketAddr> {
    let Some(addr) = sock.as_socket() else {
        fail!("Unsupported socket address {sock:?}");
    };
    Ok(addr)
}

// ADNL transport traits **********************************************

/// ADNL low level receiver
pub trait AdnlReceiver: Send {
    fn recv(&mut self) -> Result<Option<&[u8]>>;
}

/// ADNL low level sender
pub trait AdnlSender: Send {
    fn send_fast(&self, data: &[u8], addr: socket2::SockAddr) -> Result<usize>;
    fn send_safe(
        &self,
        data: Vec<u8>,
        addr: socket2::SockAddr,
        node: &Arc<AdnlNode>,
    ) -> Result<Option<usize>>;
}

// Connection-based transport *****************************************

pub(crate) struct Connections<S>(lockfree::map::Map<SocketAddr, S>);

impl<S> Connections<S> {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self(lockfree::map::Map::new()))
    }

    pub(crate) fn map(&self) -> &lockfree::map::Map<SocketAddr, S> {
        let Connections(connections) = self;
        connections
    }

    pub(crate) fn set_connection_state(
        &self,
        socket_addr: SocketAddr,
        state: impl Fn(&S) -> Result<Option<S>>,
    ) -> Result<bool> {
        add_unbound_object_to_map_with_update(self.map(), socket_addr, |found| {
            if let Some(found) = found {
                state(found)
            } else {
                Ok(None)
            }
        })
    }
}

pub(crate) struct SendQueue<Q> {
    buffer: lockfree::queue::Queue<Q>,
    sync: AtomicU8,
}

impl<Q> SendQueue<Q> {
    const SYNC_INACTIVE: u8 = 0;
    const SYNC_QUEUE_ON: u8 = 1;
    const SYNC_CHECKING: u8 = 2;

    pub(crate) fn new() -> Arc<Self> {
        Arc::new(SendQueue {
            buffer: lockfree::queue::Queue::new(),
            sync: AtomicU8::new(Self::SYNC_INACTIVE),
        })
    }

    pub(crate) fn activate(&self, on: bool) -> bool {
        let (from, to) = if on {
            (Self::SYNC_INACTIVE, Self::SYNC_QUEUE_ON)
        } else {
            (Self::SYNC_QUEUE_ON, Self::SYNC_INACTIVE)
        };
        self.switch(from, to)
    }

    pub(crate) fn check(&self, on: bool) -> bool {
        let (from, to) = if on {
            (Self::SYNC_QUEUE_ON, Self::SYNC_CHECKING)
        } else {
            (Self::SYNC_CHECKING, Self::SYNC_QUEUE_ON)
        };
        self.switch(from, to)
    }

    pub(crate) fn pop(&self) -> Option<Q> {
        self.buffer.pop()
    }

    pub(crate) fn push(&self, data: Q) {
        self.buffer.push(data);
    }

    fn switch(&self, from: u8, to: u8) -> bool {
        self.sync.compare_exchange(from, to, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    }
}

// TCP transport ******************************************************

// TCP connection establishment takes following steps:
//
// Client way:
// 0. Before send, fetch connection state by address
// 1. If no state for given address, set state to Connecting;
//    connect TCP socket to server;
//    if state is not Connecting, drop socket and go to step 0, otherwise update state to Connected;
//    send connection info to peer and wait for confirmation;
//    if state is not Connected or no confirmation, drop socket and go to step 0;
//    otherwise update state to Confirmed and go to step 3
// 2. If state is Connecting or Connected, yield execution and go to step 0
// 3. If state is Confirmed, then connection is established and send is allowed
//
// Server way:
// 0. After new connection accept, read connection info from peer
// 1. If no/bad connection info, drop socket and exit
// 2. Fetch connection state by address
// 3. If state is Confirmed, send reject, drop socket and exit
// 4. Otherwise update state to Confirmed and send confirmation
//

type TcpSendQueue = SendQueue<TcpSendContext>;

struct TcpConnection {
    queue: Arc<TcpSendQueue>,
    socket: Arc<socket2::Socket>,
}

enum TcpConnectionState {
    Confirmed(TcpConnection),
    Connected(Arc<TcpSendQueue>),
    Connecting(Arc<TcpSendQueue>),
    Disconnected(Arc<TcpSendQueue>),
}

impl Display for TcpConnectionState {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let name = match self {
            Self::Confirmed(_) => "Confirmed",
            Self::Connected(_) => "Connected",
            Self::Connecting(_) => "Connecting",
            Self::Disconnected(_) => "Disconnected",
        };
        write!(f, "{name}")
    }
}

enum TcpConnectionUpdate {
    Created(TcpStreamContext),
    Deleted(SocketAddr),
}

struct TcpStreamContext {
    buf: Vec<u8>,
    len: Option<usize>,
    offset: usize,
    peer_addr: SocketAddr,
    stream: mio::net::TcpStream,
    token: mio::Token,
}

impl TcpStreamContext {
    fn read(&mut self) -> Result<bool> {
        if self.len.is_none() {
            match self.read_len(SIZE_TCP_LENGTH) {
                Ok(true) => {
                    let len =
                        read_u32(&self.buf).map_err(|e| error!("Cannot get TCP length: {e}"))?;
                    self.len = Some(len as usize);
                    self.buf.clear();
                    self.offset = 0;
                }
                x => return x,
            }
        }
        let Some(len) = self.len else {
            return Ok(false);
        };
        self.read_len(len)
    }

    fn read_len(&mut self, len: usize) -> Result<bool> {
        self.buf.resize(len, 0);
        if self.offset >= len {
            return Ok(true);
        }
        match self.stream.read(&mut self.buf[self.offset..len]) {
            Ok(size) => {
                if size > 0 {
                    self.offset += size;
                }
                Ok(self.offset >= len)
            }
            Err(e) if e.kind() != ErrorKind::WouldBlock => {
                fail!("Cannot receive from TCP connection: {e}")
            }
            _ => Ok(false),
        }
    }
}

struct TcpReceiver {
    event_queue: VecDeque<SocketAddr>,
    events: mio::Events,
    next_token: usize,
    poll: mio::Poll,
    streams: HashMap<SocketAddr, TcpStreamContext>,
    tokens: HashMap<mio::Token, SocketAddr>,
    udp_received: bool,
    udp_token: mio::Token,
    updates: Receiver<TcpConnectionUpdate>,
}

impl TcpReceiver {
    fn drop_stream(
        &mut self,
        addr: &SocketAddr,
        mut context: Option<TcpStreamContext>,
    ) -> Result<()> {
        let mut err = if context.is_none() {
            context = self.streams.remove(addr);
            if context.is_none() {
                Some(error!("TCP recv stream for {addr} has to be deleted but not found"))
            } else {
                None
            }
        } else {
            None
        };
        if let Some(mut context) = context {
            if let Err(e) = self.poll.registry().deregister(&mut context.stream) {
                err = Some(error!("Cannot deregister TCP recv stream for {addr}: {e}"))
            }
            self.tokens.remove(&context.token);
        }
        err.map_or(Ok(()), |e| Err(e))
    }

    fn process_error(&mut self, e1: Error, addr: &SocketAddr) -> Result<bool> {
        if let Err(e2) = self.drop_stream(addr, None) {
            fail!("Cannot remove TCP recv stream to {addr} on {e1}: {e2}");
        } else {
            fail!("Removed TCP recv stream to {addr}: {e1}");
        }
    }

    fn recv_to_buf(&mut self, buf: &mut Vec<u8>) -> Result<bool> {
        loop {
            if self.udp_received {
                break Ok(false);
            }
            while let Some(addr) = self.event_queue.pop_front() {
                let Some(context) = self.streams.get_mut(&addr) else {
                    fail!("Cannot find TCP connection by address {addr}");
                };
                let Some(len) = context.len else {
                    continue;
                };
                if context.offset < len {
                    continue;
                }
                buf.clear();
                buf.extend_from_slice(&context.buf[..len]);
                context.offset = 0;
                context.len = None;
                match context.read() {
                    Ok(true) => self.event_queue.push_front(addr),
                    Ok(false) => (),
                    Err(e) => return self.process_error(e, &addr),
                }
                return Ok(true);
            }
            loop {
                let update = match self.updates.try_recv() {
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => fail!("TCP connection updates shut down"),
                    Ok(update) => update,
                };
                match update {
                    TcpConnectionUpdate::Created(mut context) => {
                        let token = mio::Token(self.next_token);
                        self.poll.registry().register(
                            &mut context.stream,
                            token,
                            mio::Interest::READABLE,
                        )?;
                        context.token = token;
                        self.tokens.insert(token, context.peer_addr);
                        let addr = context.peer_addr.clone();
                        let has_data = match context.read() {
                            Ok(has_data) => has_data,
                            Err(mut e1) => {
                                if let Err(e2) = self.drop_stream(&addr, Some(context)) {
                                    e1 = error!("{e1}; error when drop recv stream: {e2}");
                                }
                                fail!("Reject TCP connection from {addr}: {e1}");
                            }
                        };
                        if has_data {
                            self.event_queue.push_back(addr);
                        }
                        if let Some(context) = self.streams.insert(addr, context) {
                            self.drop_stream(&context.peer_addr.clone(), Some(context))?;
                        }
                        self.next_token += 1;
                        if has_data {
                            continue;
                        }
                    }
                    TcpConnectionUpdate::Deleted(addr) => self.drop_stream(&addr, None)?,
                }
            }
            self.poll.poll(&mut self.events, Some(TIMEOUT_TCP_SPIN_MS))?;
            if self.events.is_empty() {
                continue;
            }
            for event in self.events.iter() {
                if event.token() == self.udp_token {
                    self.udp_received = true;
                    continue;
                }
                let Some(addr) = self.tokens.get(&event.token()) else {
                    fail!("Cannot find TCP address by token {:?}", event.token());
                };
                let addr = *addr;
                let Some(context) = self.streams.get_mut(&addr) else {
                    fail!("Cannot find TCP connection by address {addr}");
                };
                match context.read() {
                    Ok(true) => self.event_queue.push_back(addr),
                    Ok(false) => (),
                    Err(e) => return self.process_error(e, &addr),
                }
            }
        }
    }
}

struct TcpSendContext {
    addr: socket2::SockAddr,
    data: Vec<u8>,
    len: [u8; SIZE_TCP_LENGTH],
    len_sent: bool,
    offset: usize,
    sender: Arc<TcpSender>,
    socket_addr: SocketAddr,
}

impl TcpSendContext {
    fn process_error(
        &self,
        error: Error,
        ensure_reset: bool,
        state: impl Fn(&TcpConnectionState) -> Result<Option<TcpConnectionState>>,
    ) -> Result<()> {
        let mut e1 = error!("error when send to {}: {error}", self.socket_addr);
        if let Err(e2) = self.sender.updates.send(TcpConnectionUpdate::Deleted(self.socket_addr)) {
            e1 = error!("Cannot send TCP connection reset update after {e1}: {e2}");
        }
        log::warn!(target: TARGET, "ADNL TCP error: {e1}");
        let reset = self.sender.connections.set_connection_state(self.socket_addr, state)?;
        if ensure_reset && !reset {
            fail!("Cannot reset connection state for {}", self.socket_addr);
        }
        Ok(())
    }

    fn process_error_after_confirmed(&self, error: Error) -> Result<()> {
        self.process_error(error, true, |found| {
            if let TcpConnectionState::Confirmed(conn) = found {
                Ok(Some(TcpConnectionState::Disconnected(conn.queue.clone())))
            } else {
                fail!("Cannot switch Confirmed -> Disconnected state for {}", self.socket_addr);
            }
        })
    }
}

enum TcpSendStatus {
    Failure(Error),
    Pending,
    Sent(usize),
}

struct TcpSender {
    connections: Arc<Connections<TcpConnectionState>>,
    local_ip: u32,
    local_port: u16,
    updates: Sender<TcpConnectionUpdate>,
}

impl TcpSender {
    fn confirm(
        socket: &socket2::Socket,
        context: &mut TcpStreamContext,
        local_ip: u32,
        local_port: u16,
    ) -> Result<bool> {
        let mask = local_ip ^ MASK_TCP_ADDRESS;
        let buf: [u8; SIZE_TCP_ADDRESS] = [
            (mask >> 24) as u8,
            (mask >> 16) as u8,
            (mask >> 8) as u8,
            mask as u8,
            (local_ip >> 24) as u8,
            (local_ip >> 16) as u8,
            (local_ip >> 8) as u8,
            local_ip as u8,
            (local_port >> 8) as u8,
            local_port as u8,
        ];
        let mut offset = 0;
        let start = Instant::now();
        let peer_addr = context.peer_addr;
        while !Self::send_to_socket(&socket, &buf, &mut offset)? {
            if start.elapsed() > TIMEOUT_TCP_CONNECT_MS {
                fail!("Cannot send local TCP address to peer {peer_addr}, abort connection");
            }
        }
        while !context.read_len(SIZE_TCP_CONFIRM)? {
            if start.elapsed() > TIMEOUT_TCP_CONNECT_MS {
                fail!("Cannot receive TCP confirm from peer {peer_addr}, abort connection");
            }
            thread::yield_now();
        }
        if context.buf.is_empty() {
            fail!("Cannot read TCP confirm from peer {peer_addr}, abort connection");
        }
        let ret = context.buf[0] > 0;
        context.buf.clear();
        context.offset = 0;
        Ok(ret)
    }

    fn connect(addr: &socket2::SockAddr) -> Result<(Arc<socket2::Socket>, mio::net::TcpStream)> {
        let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)?;
        configure_tcp_socket(&socket, false).map_err(|e| error!("cannot configure socket: {e}"))?;
        let mut stream = mio::net::TcpStream::from_std(
            socket.try_clone().map_err(|e| error!("cannot clone socket: {e}"))?.into(),
        );
        if let Err(e) = socket.connect(addr) {
            let wait = (e.kind() == ErrorKind::WouldBlock)
                || (e.raw_os_error() == Some(libc::EINPROGRESS));
            if !wait {
                return Err(error!("{e}"));
            }
            let token = mio::Token(0);
            let mut poll = mio::Poll::new()?;
            let mut events = mio::Events::with_capacity(1);
            poll.registry().register(&mut stream, token, mio::Interest::WRITABLE)?;
            poll.poll(&mut events, Some(TIMEOUT_TCP_CONNECT_MS))?;
            poll.registry().deregister(&mut stream)?;
            let mut ok = false;
            for event in events.iter() {
                if (event.token() == token) && event.is_writable() {
                    ok = true;
                    break;
                }
            }
            if !ok {
                fail!("connection timed out");
            }
        }
        Ok((Arc::new(socket), stream))
    }

    fn send(
        self: &Arc<Self>,
        data: Vec<u8>,
        addr: socket2::SockAddr,
        node: &Arc<AdnlNode>,
    ) -> Result<Option<usize>> {
        let len = data.len();
        let len: [u8; SIZE_TCP_LENGTH] =
            [(len >> 24) as u8, (len >> 16) as u8, (len >> 8) as u8, len as u8];
        let socket_addr = sock_to_socket_addr(&addr)?;
        let mut ctx = TcpSendContext {
            addr,
            data,
            len,
            len_sent: false,
            offset: 0,
            sender: self.clone(),
            socket_addr,
        };
        let connections = ctx.sender.connections.clone();
        let (queue, activate) = loop {
            match connections.map().get(&socket_addr) {
                None => {
                    // Create send queue in single thread context
                    let queue = TcpSendQueue::new();
                    if !add_unbound_object_to_map(connections.map(), socket_addr, || {
                        Ok(TcpConnectionState::Disconnected(queue.clone()))
                    })? {
                        continue;
                    }
                    break (queue, true);
                }
                Some(state) => {
                    match state.val() {
                        TcpConnectionState::Confirmed(conn) => {
                            // Queue is pushed only in single thread context, so this check is ok
                            if conn.queue.check(true) {
                                conn.queue.push(ctx);
                                while !conn.queue.check(false) {
                                    thread::yield_now();
                                }
                            } else {
                                match Self::send_step(&conn.socket, &mut ctx) {
                                    TcpSendStatus::Failure(e) => {
                                        ctx.process_error_after_confirmed(e)?;
                                        break (conn.queue.clone(), true);
                                    }
                                    TcpSendStatus::Pending => break (conn.queue.clone(), true),
                                    TcpSendStatus::Sent(len) => return Ok(Some(len)),
                                }
                            }
                        }
                        TcpConnectionState::Connected(queue)
                        | TcpConnectionState::Connecting(queue)
                        | TcpConnectionState::Disconnected(queue) => {
                            if queue.activate(true) {
                                break (queue.clone(), false);
                            } else {
                                queue.push(ctx);
                            }
                        }
                    }
                }
            };
            return Ok(None);
        };
        queue.push(ctx);
        // queue.sync.store(TcpSendQueue::SYNC_INACTIVE, Ordering::Relaxed);
        let node = node.clone();
        let name = format!("{socket_addr} ADNL TCP pending send");
        thread::Builder::new().name(name).spawn(move || {
            if activate {
                while !queue.activate(true) {
                    thread::yield_now();
                }
            }
            loop {
                let Some(ctx) = queue.pop() else {
                    if queue.activate(false) {
                        break;
                    }
                    thread::yield_now();
                    continue;
                };
                let len = ctx.data.len();
                node.after_send(Self::send_loop(ctx), len)
            }
        })?;
        Ok(None)
    }

    fn send_loop(mut ctx: TcpSendContext) -> Result<usize> {
        let socket_addr = ctx.socket_addr;
        let connections = ctx.sender.connections.clone();
        let convert_err = |e, msg| error!("error when {msg} to {socket_addr}: {e}");
        loop {
            match connections.map().get(&socket_addr) {
                None => fail!("Cannot create send queue to {socket_addr} in background"),
                Some(state) => match state.val() {
                    TcpConnectionState::Confirmed(conn) => {
                        match Self::send_step(&conn.socket, &mut ctx) {
                            TcpSendStatus::Failure(e) => {
                                ctx.process_error_after_confirmed(e)?;
                                continue;
                            }
                            TcpSendStatus::Pending => continue,
                            TcpSendStatus::Sent(len) => break Ok(len),
                        }
                    }
                    TcpConnectionState::Disconnected(_) => (),
                    x => fail!("Wrong connection state {x} for {socket_addr} in background"),
                },
            }
            if !ctx.sender.connections.set_connection_state(ctx.socket_addr, |found| {
                if let TcpConnectionState::Disconnected(queue) = found {
                    Ok(Some(TcpConnectionState::Connecting(queue.clone())))
                } else if let TcpConnectionState::Confirmed(_) = found {
                    Ok(None)
                } else {
                    fail!("Cannot switch {found} -> Connecting state for {socket_addr}");
                }
            })? {
                continue;
            }
            log::info!(target: TARGET, "Try new ADNL TCP connection to {socket_addr}");
            let result = Self::connect(&ctx.addr).map_err(|e| convert_err(e, "connect"));
            let (socket, stream) = match result {
                Err(e) => {
                    ctx.process_error(e, false, |found| {
                        if let TcpConnectionState::Connecting(queue) = found {
                            Ok(Some(TcpConnectionState::Disconnected(queue.clone())))
                        } else if let TcpConnectionState::Confirmed(_) = found {
                            Ok(None)
                        } else {
                            fail!("Cannot switch {found} -> Disconnected state for {socket_addr}");
                        }
                    })?;
                    continue;
                }
                Ok(ok) => ok,
            };
            if !ctx.sender.connections.set_connection_state(ctx.socket_addr, |found| {
                if let TcpConnectionState::Connecting(queue) = found {
                    Ok(Some(TcpConnectionState::Connected(queue.clone())))
                } else if let TcpConnectionState::Confirmed(_) = found {
                    Ok(None)
                } else {
                    fail!("Cannot switch {found} -> Connected state for {socket_addr}");
                }
            })? {
                continue;
            }
            log::info!(target: TARGET, "ADNL TCP connected to {socket_addr}");
            let mut stream_context = TcpStreamContext {
                buf: Vec::new(),
                len: None,
                offset: 0,
                peer_addr: socket_addr,
                stream,
                token: mio::Token(0),
            };
            let result = Self::confirm(
                &socket,
                &mut stream_context,
                ctx.sender.local_ip,
                ctx.sender.local_port,
            )
            .map_err(|e| convert_err(e, "confirm"));
            let confirmed = match result {
                Err(e) => {
                    ctx.process_error(e, false, |found| {
                        if let TcpConnectionState::Connected(queue) = found {
                            Ok(Some(TcpConnectionState::Disconnected(queue.clone())))
                        } else if let TcpConnectionState::Confirmed(_) = found {
                            Ok(None)
                        } else {
                            fail!("Cannot switch {found} -> Disconnected state for {socket_addr}");
                        }
                    })?;
                    continue;
                }
                Ok(confirmed) => confirmed,
            };
            if !ctx.sender.connections.set_connection_state(ctx.socket_addr, |found| {
                if let TcpConnectionState::Connected(queue) = found {
                    if confirmed {
                        let conn = TcpConnection { queue: queue.clone(), socket: socket.clone() };
                        Ok(Some(TcpConnectionState::Confirmed(conn)))
                    } else {
                        Ok(Some(TcpConnectionState::Disconnected(queue.clone())))
                    }
                } else if let TcpConnectionState::Confirmed(_) = found {
                    Ok(None)
                } else {
                    fail!("Cannot switch {found} -> Confirmed state for {socket_addr}");
                }
            })? {
                continue;
            }
            if confirmed {
                log::info!(target: TARGET, "Confirmed ADNL TCP connection to {socket_addr}");
                ctx.sender
                    .updates
                    .send(TcpConnectionUpdate::Created(stream_context))
                    .map_err(|e| convert_err(e.into(), "TCP connection update"))?;
            }
        }
    }

    fn send_step(socket: &socket2::Socket, ctx: &mut TcpSendContext) -> TcpSendStatus {
        if !ctx.len_sent {
            match Self::send_to_socket(socket, &ctx.len, &mut ctx.offset) {
                Ok(false) => return TcpSendStatus::Pending,
                Ok(true) => (),
                Err(e) => return TcpSendStatus::Failure(e),
            }
            ctx.len_sent = true;
            ctx.offset = 0;
        }
        match Self::send_to_socket(socket, &ctx.data, &mut ctx.offset) {
            Ok(false) => TcpSendStatus::Pending,
            Ok(true) => TcpSendStatus::Sent(ctx.data.len()),
            Err(e) => {
                ctx.len_sent = false;
                ctx.offset = 0;
                TcpSendStatus::Failure(e)
            }
        }
    }

    fn send_to_socket(socket: &socket2::Socket, data: &[u8], offset: &mut usize) -> Result<bool> {
        match socket.send(&data[*offset..]) {
            Ok(size) => {
                *offset += size;
                let len = data.len();
                if *offset < len {
                    thread::yield_now();
                    Ok(false)
                } else if *offset > len {
                    fail!("Error sending TCP data: sent {offset} bytes of {len}");
                } else {
                    Ok(true)
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                thread::yield_now();
                Ok(false)
            }
            Err(err) => Err(error!("Error when send to TCP socket: {err}")),
        }
    }
}

fn tcp_sender_receiver(node: &Arc<AdnlNode>) -> Result<(Arc<TcpSender>, TcpReceiver)> {
    fn listen(listener: &socket2::Socket, sender: &Arc<TcpSender>) -> Result<()> {
        let (socket, peer_addr) = match listener.accept() {
            Ok((socket, peer_addr)) => {
                let peer_addr = sock_to_socket_addr(&peer_addr)?;
                (socket, peer_addr)
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(TIMEOUT_TCP_SPIN_MS);
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let sender = sender.clone();
        let name = format!("{} ADNL TCP accept", peer_addr);
        thread::Builder::new().name(name).spawn(move || {
            if let Err(e) = accept(socket, peer_addr, sender) {
                log::warn!(target: TARGET, "ERROR in ADNL TCP acceptor: {e}");
            }
        })?;
        Ok(())
    }

    fn accept(
        socket: socket2::Socket,
        peer_addr: SocketAddr,
        sender: Arc<TcpSender>,
    ) -> Result<()> {
        log::info!(target: TARGET, "Incoming TCP connection from {}", peer_addr);
        configure_tcp_socket(&socket, false)?;
        let socket = Arc::new(socket);
        let mut context = TcpStreamContext {
            buf: Vec::new(),
            len: None,
            offset: 0,
            peer_addr,
            stream: mio::net::TcpStream::from_std(socket.try_clone()?.into()),
            token: mio::Token(0),
        };
        let start = Instant::now();
        while !context.read_len(SIZE_TCP_ADDRESS)? {
            if start.elapsed() > TIMEOUT_TCP_CONNECT_MS {
                fail!("Cannot read TCP address of peer {}, abort connection", context.peer_addr);
            }
            thread::yield_now();
        }
        let mask =
            read_u32(&context.buf).map_err(|e| error!("Cannot read TCP IP address mask: {e}"))?;
        let peer_ip =
            read_u32(&context.buf[4..]).map_err(|e| error!("Cannot read TCP IP address: {e}"))?;
        if (mask ^ peer_ip) != MASK_TCP_ADDRESS {
            fail!(
                "TCP connection from peer {} with incompatible protocol, abort connection",
                context.peer_addr
            );
        }
        let peer_port =
            read_u16(&context.buf[8..]).map_err(|e| error!("Cannot read TCP port: {e}"))?;
        context.buf.clear();
        context.offset = 0;
        context.peer_addr.set_ip(IpAddr::V4(Ipv4Addr::from(peer_ip)));
        context.peer_addr.set_port(peer_port);
        let peer_addr = context.peer_addr;
        let queue = TcpSendQueue::new();
        let confirmed =
            add_unbound_object_to_map_with_update(sender.connections.map(), peer_addr, |found| {
                let confirm = |queue: &Arc<TcpSendQueue>| {
                    let conn = TcpConnection { queue: queue.clone(), socket: socket.clone() };
                    Some(TcpConnectionState::Confirmed(conn))
                };
                match found {
                    None => Ok(confirm(&queue)),
                    Some(TcpConnectionState::Confirmed(_)) => Ok(None),
                    Some(TcpConnectionState::Connected(queue)) => Ok(confirm(queue)),
                    Some(TcpConnectionState::Connecting(queue)) => Ok(confirm(queue)),
                    Some(TcpConnectionState::Disconnected(queue)) => Ok(confirm(queue)),
                }
            })?;
        // Respond with non-zero/zero byte
        let confirmed_tag = if confirmed { (peer_ip >> 24) as u8 | 1 } else { 0 };
        let buf: [u8; SIZE_TCP_CONFIRM] = [confirmed_tag];
        let mut offset = 0;
        let start = Instant::now();
        while !TcpSender::send_to_socket(&socket, &buf, &mut offset)? {
            if start.elapsed() > TIMEOUT_TCP_CONNECT_MS {
                fail!("Cannot send TCP confirmation to peer {peer_addr}, abort connection");
            }
        }
        if confirmed {
            log::info!(target: TARGET, "ADNL TCP connection from {peer_addr} accepted");
            sender
                .updates
                .send(TcpConnectionUpdate::Created(context))
                .map_err(|e| error!("error when update TCP connection from {peer_addr}: {e}"))?;
        } else {
            log::info!(target: TARGET, "ADNL TCP connection from {peer_addr} rejected");
        }
        Ok(())
    }

    let listener = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)?;
    listener.set_nonblocking(true)?;
    listener.set_reuse_address(true)?;
    let local = node.config().ip_address();
    let local_ip = local.ip();
    let local_port = local.port();
    listener.bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), local_port).into())?;
    listener.listen(SOCKET_TCP_BACKLOG as i32)?;
    log::info!(target: TARGET, "ADNL TCP listening on {local}...");
    let (updates_sender, updates_receiver) = channel();
    let receiver = TcpReceiver {
        event_queue: VecDeque::new(),
        events: mio::Events::with_capacity(SOCKET_TCP_BACKLOG),
        next_token: 1,
        poll: mio::Poll::new()?,
        streams: HashMap::new(),
        tokens: HashMap::new(),
        udp_received: false,
        udp_token: mio::Token(0),
        updates: updates_receiver,
    };
    let sender = Arc::new(TcpSender {
        connections: Connections::new(),
        local_ip,
        local_port,
        updates: updates_sender.clone(),
    });
    let sender_context = sender.clone();
    let stop = node.stopper().clone();
    thread::Builder::new().name("ADNL TCP listener".into()).spawn(move || {
        stop.acquire(AdnlNode::MASK_TRANSPORT);
        loop {
            if stop.is_stopped() {
                break;
            }
            if let Err(e) = listen(&listener, &sender_context) {
                log::warn!(target: TARGET, "ERROR in ADNL TCP listener: {e}");
            }
        }
        stop.release(AdnlNode::MASK_TRANSPORT);
        log::info!(target: TARGET, "Stopped ADNL TCP listening");
    })?;
    Ok((sender, receiver))
}

// UDP transport ******************************************************

struct UdpTcpReceiver {
    buf_tcp: Vec<u8>,
    buf_udp: [u8; SIZE_UDP_BUFFER],
    tcp: Option<TcpReceiver>,
    udp: mio::net::UdpSocket,
}

impl AdnlReceiver for UdpTcpReceiver {
    fn recv(&mut self) -> Result<Option<&[u8]>> {
        fn recv_udp<'a>(
            udp: &'a mio::net::UdpSocket,
            buf: &'a mut [u8],
        ) -> Result<Option<&'a [u8]>> {
            match udp.recv_from(buf) {
                Ok((len, _)) => {
                    if len == 0 {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[..len]))
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    thread::yield_now();
                    Ok(None)
                }
                Err(err) => Err(error!("Error when receiving from UDP: {err}")),
            }
        }

        if let Some(tcp) = &mut self.tcp {
            if tcp.recv_to_buf(&mut self.buf_tcp)? {
                Ok(Some(&self.buf_tcp[..]))
            } else if !tcp.udp_received {
                Ok(None)
            } else if let Some(data) = recv_udp(&self.udp, &mut self.buf_udp)? {
                Ok(Some(data))
            } else {
                tcp.udp_received = false;
                Ok(None)
            }
        } else {
            recv_udp(&self.udp, &mut self.buf_udp)
        }
    }
}

struct UdpTcpSender {
    tcp: Option<Arc<TcpSender>>,
    udp: socket2::Socket,
}

impl AdnlSender for UdpTcpSender {
    fn send_fast(&self, data: &[u8], addr: socket2::SockAddr) -> Result<usize> {
        loop {
            match self.udp.send_to(data, &addr) {
                Ok(usize) => break Ok(usize),
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    thread::yield_now();
                    continue;
                }
                Err(err) => break Err(error!("Error when sending to UDP socket: {err}")),
            }
        }
    }
    fn send_safe(
        &self,
        data: Vec<u8>,
        addr: socket2::SockAddr,
        node: &Arc<AdnlNode>,
    ) -> Result<Option<usize>> {
        let Some(tcp) = &self.tcp else {
            fail!("TCP sender is not available");
        };
        tcp.send(data, addr, node)
    }
}

fn create_sender_receiver(
    node: &Arc<AdnlNode>,
    udp_only: bool,
) -> Result<(Box<dyn AdnlSender>, Box<dyn AdnlReceiver>)> {
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)?;
    // socket_recv.set_send_buffer_size(1 << 26)?;
    socket.set_recv_buffer_size(SOCKET_BUFFER_SIZE)?;
    socket.set_reuse_address(true)?;
    if !udp_only {
        socket.set_nonblocking(true)?;
    }
    let address = node.config().ip_address();
    socket.bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), address.port()).into())?;
    log::info!(target: TARGET, "ADNL UDP receiving on {address}...");
    let (tcp_sender, mut tcp_receiver) = if udp_only {
        (None, None)
    } else {
        let (tcp_sender, tcp_receiver) = tcp_sender_receiver(node)?;
        (Some(tcp_sender), Some(tcp_receiver))
    };
    let sender = UdpTcpSender { tcp: tcp_sender, udp: socket.try_clone()? };
    let mut udp = mio::net::UdpSocket::from_std(socket.into());
    if let Some(tcp_receiver) = &mut tcp_receiver {
        tcp_receiver.poll.registry().register(
            &mut udp,
            tcp_receiver.udp_token,
            mio::Interest::READABLE,
        )?;
    }
    let receiver = UdpTcpReceiver {
        buf_tcp: Vec::new(),
        buf_udp: [0u8; SIZE_UDP_BUFFER],
        tcp: tcp_receiver,
        udp,
    };
    Ok((Box::new(sender), Box::new(receiver)))
}

pub(crate) fn udp_sender_receiver(
    node: &Arc<AdnlNode>,
) -> Result<(Box<dyn AdnlSender>, Box<dyn AdnlReceiver>)> {
    create_sender_receiver(node, true)
}

pub(crate) fn udp_tcp_sender_receiver(
    node: &Arc<AdnlNode>,
) -> Result<(Box<dyn AdnlSender>, Box<dyn AdnlReceiver>)> {
    create_sender_receiver(node, false)
}
