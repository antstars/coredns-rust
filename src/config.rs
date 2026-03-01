//! Configuration parsing for CoreDNS

use crate::plugin::{create_plugin, SharedState, Plugin};
use anyhow::Result;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct PluginConfig {
    pub name: String,
    pub args: Vec<String>,
    pub block: Vec<PluginConfig>,
}

pub struct Config {
    pub zones: Vec<ZoneConfig>,
}

pub struct ZoneConfig {
    pub name: String,
    pub plugins: Vec<Box<dyn Plugin>>,
}

#[derive(Debug, PartialEq)]
enum Token { Text(String), OpenBrace, CloseBrace, Newline }

struct RawZone { name: String, plugins: Vec<PluginConfig> }

impl Config {
    /// Load configuration from a file path
    pub fn load(path: &str, shared: Arc<SharedState>) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file '{}': {}", path, e))?;
        Self::parse(&content, shared)
    }

    pub fn parse(content: &str, shared: Arc<SharedState>) -> Result<Self> {
        let tokens = Self::lex(content);
        let raw_zones = Self::parse_tokens(&tokens)?;
        let mut zones = Vec::new();
        
        for raw in raw_zones {
            let mut plugins = Vec::new();
            for p_cfg in &raw.plugins {
                if let Ok(plugin) = create_plugin(p_cfg, shared.clone()) {
                    plugins.push(plugin);
                }
            }
            
            // 【核心修复】：严格遵守 CoreDNS 规范！
            // 插件的执行顺序必须由内置的 Priority 决定，与 Corefile 书写顺序无关。
            // 按照优先级从大到小排序 (比如 Cache:120 必须在 Forward:100 之前拦截执行)
            plugins.sort_by(|a, b| b.priority().cmp(&a.priority()));
            
            zones.push(ZoneConfig { name: raw.name, plugins });
        }
        Ok(Config { zones })
    }

    fn lex(input: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut chars = input.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c == '\n' { tokens.push(Token::Newline); chars.next(); } 
            else if c.is_whitespace() { chars.next(); } 
            else if c == '#' { while let Some(&c) = chars.peek() { if c == '\n' { break; } chars.next(); } } 
            else if c == '{' { tokens.push(Token::OpenBrace); chars.next(); } 
            else if c == '}' { tokens.push(Token::CloseBrace); chars.next(); } 
            else if c == '"' {
                chars.next(); 
                let mut s = String::new();
                while let Some(&c) = chars.peek() { if c == '"' { chars.next(); break; } s.push(c); chars.next(); }
                tokens.push(Token::Text(s));
            } else {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '#' || c == '{' || c == '}' || c == '"' { break; }
                    s.push(c); chars.next();
                }
                tokens.push(Token::Text(s));
            }
        }
        tokens
    }

    fn parse_tokens(tokens: &[Token]) -> Result<Vec<RawZone>> {
        let mut i = 0;
        let mut zones = Vec::new();
        let mut zone_names = Vec::new();
        while i < tokens.len() {
            match &tokens[i] {
                Token::Text(s) => { zone_names.push(s.clone()); i += 1; }
                Token::OpenBrace => {
                    i += 1;
                    let (plugins, next_i) = Self::parse_block(tokens, i)?;
                    i = next_i;
                    for name in zone_names.drain(..) { zones.push(RawZone { name, plugins: plugins.clone() }); }
                }
                Token::Newline => { i += 1; zone_names.clear(); }
                Token::CloseBrace => { i += 1; }
            }
        }
        Ok(zones)
    }

    /// Parse a configuration block starting at position i
    fn parse_block(tokens: &[Token], mut i: usize) -> Result<(Vec<PluginConfig>, usize)> {
        let mut plugins = Vec::new();
        while i < tokens.len() {
            match &tokens[i] {
                Token::Newline => { i += 1; }
                Token::CloseBrace => { i += 1; return Ok((plugins, i)); }
                Token::Text(name) => {
                    let plugin_name = name.clone(); i += 1;
                    let mut args = Vec::new();
                    let mut block = Vec::new();
                    while i < tokens.len() {
                        match &tokens[i] {
                            Token::Text(arg) => { args.push(arg.clone()); i += 1; }
                            Token::OpenBrace => {
                                i += 1;
                                let (sub_block, next_i) = Self::parse_block(tokens, i)?;
                                block = sub_block; i = next_i; break;
                            }
                            Token::Newline | Token::CloseBrace => { break; }
                        }
                    }
                    plugins.push(PluginConfig { name: plugin_name, args, block });
                }
                _ => { i += 1; }
            }
        }
        Ok((plugins, i))
    }
}