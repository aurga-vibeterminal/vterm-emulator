use std::collections::HashSet;

use super::*;

impl Terminal {
    pub fn set_sequence_callback<F>(&mut self, types: HashSet<String>, callback: F)
    where
        F: FnMut(String, String, Vec<u8>) + Send + Sync + 'static,
    {
        self.sequence_callback_types = types;
        self.sequence_callback = Some(Box::new(callback));
    }

    pub fn clear_sequence_callback(&mut self) {
        self.sequence_callback = None;
        self.sequence_callback_types.clear();
    }

    pub(super) fn emit_sequence(&mut self, seq_type: &str, code: String, data: Vec<u8>) {
        if !self.sequence_callback_types.contains(seq_type) {
            return;
        }
        if let Some(callback) = self.sequence_callback.as_mut() {
            callback(seq_type.to_string(), code, data);
        }
    }

    pub(super) fn scan_apc_sequences(&mut self, data: &[u8]) {
        for &byte in data {
            if self.apc_parser.in_apc {
                if self.apc_parser.escape_pending {
                    if byte == b'\\' {
                        let data = std::mem::take(&mut self.apc_parser.data);
                        self.apc_parser.in_apc = false;
                        self.apc_parser.escape_pending = false;
                        self.emit_sequence("apc", String::new(), data);
                    } else {
                        self.apc_parser.data.push(0x1B);
                        self.apc_parser.data.push(byte);
                        self.apc_parser.escape_pending = false;
                    }
                    continue;
                }

                if byte == 0x1B {
                    self.apc_parser.escape_pending = true;
                } else if byte == 0x9C {
                    let data = std::mem::take(&mut self.apc_parser.data);
                    self.apc_parser.in_apc = false;
                    self.emit_sequence("apc", String::new(), data);
                } else {
                    self.apc_parser.data.push(byte);
                }
                continue;
            }

            if self.apc_parser.escape_pending {
                self.apc_parser.escape_pending = false;
                if byte == b'_' {
                    self.apc_parser.in_apc = true;
                    self.apc_parser.data.clear();
                } else if byte == 0x1B {
                    self.apc_parser.escape_pending = true;
                }
                continue;
            }

            if byte == 0x1B {
                self.apc_parser.escape_pending = true;
            } else if byte == 0x9F {
                self.apc_parser.in_apc = true;
                self.apc_parser.data.clear();
            }
        }
    }

    /// Get a cell at the given position
    pub fn process(&mut self, data: &[u8]) {
        let decoded = self.input_decoder.decode(data);
        let data = decoded.as_deref().unwrap_or(data);
        // The APC scan is an extra byte-level pass over the whole chunk whose
        // results are only consumed by sequence callbacks registered for
        // "apc". Skip it entirely when nobody is listening (the common case).
        if self.sequence_callback_types.contains("apc") {
            self.scan_apc_sequences(data);
        } else if self.apc_parser.in_apc || self.apc_parser.escape_pending {
            // Drop any half-scanned APC sequence so a later registration does
            // not emit a payload stitched across the un-scanned gap.
            self.apc_parser.in_apc = false;
            self.apc_parser.escape_pending = false;
            self.apc_parser.data.clear();
        }

        if let Some(mut parser) = self.parser.take() {
            let mut filter = std::mem::take(&mut self.eol_mark_filter);
            filter.feed(self, &mut parser, data);
            self.eol_mark_filter = filter;
            self.parser = Some(parser);
        }
    }
}
