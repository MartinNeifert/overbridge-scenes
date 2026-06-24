//! MIDI CC → parameter mapping tests.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::host::HostCommand;
use crate::midi::{MapperConfig, MidiMapper, MidiMapping, MidiSource, MappingTarget};

fn mapper_with_cc(channel: u8, controller: u8, param: &str) -> MidiMapper {
    let config = MapperConfig {
        mappings: vec![MidiMapping {
            source: MidiSource::Cc { channel, controller },
            target: MappingTarget {
                parameter: param.to_string(),
                curve: "linear".to_string(),
            },
        }],
        ..Default::default()
    };
    let mut index = HashMap::new();
    index.insert(param.to_ascii_lowercase(), 0usize);
    MidiMapper::new(config, Arc::new(RwLock::new(index)))
}

#[test]
fn cc_maps_to_set_parameter_by_name() {
    let mapper = mapper_with_cc(0, 74, "Filter Cutoff");
    let cmd = mapper
        .translate(&[0xB0, 74, 127])
        .expect("CC should map");
    match cmd {
        HostCommand::SetParameterByName { name, value } => {
            assert_eq!(name, "Filter Cutoff");
            assert!((value - 1.0).abs() < 1e-6);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn cc_respects_midi_channel() {
    let mapper = mapper_with_cc(1, 10, "Drive");
    assert!(mapper.translate(&[0xB0, 10, 64]).is_none());
    let cmd = mapper
        .translate(&[0xB1, 10, 64])
        .expect("channel 1 CC");
    match cmd {
        HostCommand::SetParameterByName { name, value } => {
            assert_eq!(name, "Drive");
            assert!((value - 64.0 / 127.0).abs() < 1e-4);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn unmapped_cc_returns_none() {
    let mapper = mapper_with_cc(0, 1, "Drive");
    assert!(mapper.translate(&[0xB0, 99, 64]).is_none());
}

#[test]
fn note_source_not_translated_to_parameter() {
    let config = MapperConfig {
        mappings: vec![MidiMapping {
            source: MidiSource::Note {
                channel: 0,
                note: 60,
            },
            target: MappingTarget {
                parameter: "Drive".to_string(),
                curve: "linear".to_string(),
            },
        }],
        ..Default::default()
    };
    let index = Arc::new(RwLock::new(HashMap::new()));
    let mapper = MidiMapper::new(config, index);
    assert!(mapper.translate(&[0x90, 60, 100]).is_none());
}

#[test]
fn forward_raw_wraps_bytes() {
    let mapper = mapper_with_cc(0, 1, "Drive");
    let cmd = mapper
        .forward_raw(&[0xF0, 0x7E])
        .expect("raw midi");
    match cmd {
        HostCommand::SendRawMidi { data } => assert_eq!(data, vec![0xF0, 0x7E]),
        other => panic!("unexpected command: {other:?}"),
    }
}
