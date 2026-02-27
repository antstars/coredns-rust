//! Basic types for CoreDNS

use std::net::Ipv4Addr;
use std::net::SocketAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QType { A, AAAA, MX, TXT, CNAME, NS, SOA, PTR, SRV, ANY }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QClass { IN, CH, HS }

#[derive(Debug, Clone)]
pub enum Record {
    A { addr: Ipv4Addr },
    AAAA { addr: std::net::Ipv6Addr },
    TXT { text: Vec<String> },
    CNAME { cname: String },
    MX { preference: u16, exchange: String },
    NS { nsdname: String },
    SOA { mname: String, rname: String, serial: u32, refresh: u32, retry: u32, expire: u32, minimum: u32 },
    PTR { ptrdname: String },
    SRV { priority: u16, weight: u16, port: u16, target: String },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DnsHeader {
    pub id: u16, pub flags: HeaderFlags,
    pub question_count: u16, pub answer_count: u16,
    pub authority_count: u16, pub additional_count: u16,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HeaderFlags {
    pub qr: bool, pub opcode: u8, pub aa: bool, pub tc: bool,
    pub rd: bool, pub ra: bool, pub rcode: u8,
}

#[derive(Debug, Clone)]
pub struct DnsQuestion { pub name: String, pub qtype: QType, pub qclass: QClass }

#[derive(Debug, Clone, Default)]
pub struct DnsMessage {
    pub header: DnsHeader,
    pub questions: Vec<DnsQuestion>,
    pub answers: Vec<Record>,
    pub authority: Vec<Record>,
    pub additional: Vec<Record>,
    
    pub raw_query: Vec<u8>,
    pub raw_response: Option<Vec<u8>>,
    pub halt_chain: bool,

    pub client_addr: Option<SocketAddr>,
    pub protocol: String,
    
    // --- 【监控上下文】 ---
    pub server_port: Option<u16>,
    pub start_time: Option<std::time::Instant>,
    pub answered_by: String, // 记录是哪个插件(如 "cache", "forward")响应的
}