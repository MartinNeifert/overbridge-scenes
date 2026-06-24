//! Minimal VST3 host-side COM services required by plugins such as
//! Elektron Overbridge that expect a real host context and sample-
//! accurate parameter delivery through `IParameterChanges`.
//!
//! Every plugin→host callback is logged at `info` via [`log_vst_handler!`] so
//! device operations can be traced with `RUST_LOG=info` (or `RUST_LOG=vst_handler=info`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use vst3::Steinberg::Vst::{
    IComponentHandler, IComponentHandler2, IComponentHandler2Trait, IComponentHandlerTrait,
    IHostApplication, IHostApplicationTrait, IParamValueQueue, IParamValueQueueTrait,
    IParameterChanges, IParameterChangesTrait, IUnitHandler, IUnitHandler2, IUnitHandler2Trait,
    IUnitHandlerTrait, ParamID, ParamValue, ProgramListID, RestartFlags_, String128, UnitID,
};
use vst3::Steinberg::{FIDString, TUID, kNotImplemented, kResultOk, tresult};
use vst3::{Class, ComWrapper};

/// Log an incoming plugin→host VST callback. Add this to every host-side handler method.
#[macro_export]
macro_rules! log_vst_handler {
    ($iface:expr, $method:expr $(, $key:ident = $val:expr)*) => {
        tracing::info!(
            target: "vst_handler",
            iface = $iface,
            method = $method
            $(, $key = $val)*
        );
    };
}

fn decode_fid_string(name: FIDString) -> String {
    if name.is_null() {
        return "(null)".into();
    }
    unsafe {
        std::ffi::CStr::from_ptr(name.cast())
            .to_string_lossy()
            .into_owned()
    }
}

fn decode_restart_flags(flags: i32) -> String {
    const FLAGS: &[(i32, &str)] = &[
        (RestartFlags_::kReloadComponent, "kReloadComponent"),
        (RestartFlags_::kIoChanged, "kIoChanged"),
        (RestartFlags_::kParamValuesChanged, "kParamValuesChanged"),
        (RestartFlags_::kLatencyChanged, "kLatencyChanged"),
        (RestartFlags_::kParamTitlesChanged, "kParamTitlesChanged"),
        (RestartFlags_::kMidiCCAssignmentChanged, "kMidiCCAssignmentChanged"),
        (RestartFlags_::kNoteExpressionChanged, "kNoteExpressionChanged"),
        (RestartFlags_::kIoTitlesChanged, "kIoTitlesChanged"),
        (RestartFlags_::kPrefetchableSupportChanged, "kPrefetchableSupportChanged"),
        (RestartFlags_::kRoutingInfoChanged, "kRoutingInfoChanged"),
        (RestartFlags_::kKeyswitchChanged, "kKeyswitchChanged"),
        (RestartFlags_::kParamIDMappingChanged, "kParamIDMappingChanged"),
    ];
    let labels: Vec<&str> = FLAGS
        .iter()
        .filter(|(mask, _)| flags & mask != 0)
        .map(|(_, label)| *label)
        .collect();
    if labels.is_empty() {
        format!("0x{flags:x}")
    } else {
        labels.join("|")
    }
}

static EDITOR_OPEN_NOTIFIER: Mutex<Option<Sender<()>>> = Mutex::new(None);
static PARAM_CHANGE_NOTIFIER: Mutex<Option<Sender<(ParamID, ParamValue)>>> = Mutex::new(None);
static PARAM_REFRESH_NOTIFIER: Mutex<Option<Sender<()>>> = Mutex::new(None);

struct HardwareEditState {
    last_edit: Option<Instant>,
    values: HashMap<ParamID, ParamValue>,
}

fn hardware_edits() -> &'static Mutex<HardwareEditState> {
    static EDITS: OnceLock<Mutex<HardwareEditState>> = OnceLock::new();
    EDITS.get_or_init(|| {
        Mutex::new(HardwareEditState {
            last_edit: None,
            values: HashMap::new(),
        })
    })
}

/// True while the plugin is streaming `performEdit` from hardware (knob turns).
pub fn hardware_edit_active(within: Duration) -> bool {
    hardware_edits()
        .lock()
        .expect("hardware edits lock")
        .last_edit
        .is_some_and(|t| t.elapsed() < within)
}

/// Latest normalized values from `performEdit`, keyed by VST param id.
pub fn recent_hardware_values() -> HashMap<ParamID, ParamValue> {
    hardware_edits()
        .lock()
        .expect("hardware edits lock")
        .values
        .clone()
}

/// Clear after a preset-style state change so polling reads fresh values.
pub fn clear_hardware_edits() {
    let mut state = hardware_edits().lock().expect("hardware edits lock");
    state.values.clear();
    state.last_edit = None;
}

/// Timestamp of the most recent host-initiated parameter set (UI / MIDI / macro).
static HOST_EDIT: Mutex<Option<Instant>> = Mutex::new(None);

/// Record that the host just set a parameter. Used to suppress preset-load
/// detection so the host's own edits aren't misread as a device preset change
/// (which would trigger an expensive full-state refresh and fight the edit).
pub fn note_host_edit() {
    *HOST_EDIT.lock().expect("host edit lock") = Some(Instant::now());
}

/// True if the host set a parameter within `within`.
pub fn host_edit_active(within: Duration) -> bool {
    HOST_EDIT
        .lock()
        .expect("host edit lock")
        .is_some_and(|t| t.elapsed() < within)
}

/// Register a channel notified when a plugin calls `IComponentHandler2::requestOpenEditor`.
/// Safe to call again when switching plugins.
pub fn set_editor_open_notifier(tx: Sender<()>) {
    *EDITOR_OPEN_NOTIFIER.lock().expect("editor notifier lock") = Some(tx);
}

/// Register a channel notified when the plugin calls `IComponentHandler::performEdit`
/// (hardware knob, preset load, etc.).
pub fn set_param_change_notifier(tx: Sender<(ParamID, ParamValue)>) {
    *PARAM_CHANGE_NOTIFIER.lock().expect("param change notifier lock") = Some(tx);
}

/// Register a channel notified when all parameter values may have changed
/// (preset load, `restartComponent`, group edit end, etc.).
pub fn set_param_refresh_notifier(tx: Sender<()>) {
    *PARAM_REFRESH_NOTIFIER.lock().expect("param refresh notifier lock") = Some(tx);
}

fn notify_editor_open_request() {
    if let Some(tx) = EDITOR_OPEN_NOTIFIER.lock().expect("editor notifier lock").as_ref() {
        let _ = tx.try_send(());
    }
}

/// Inject a hardware-style `performEdit` (used by the in-process test plugin).
pub fn simulate_perform_edit(id: ParamID, value: ParamValue) {
    notify_param_change(id, value);
}

fn notify_param_change(id: ParamID, value: ParamValue) {
    {
        let mut hw = hardware_edits().lock().expect("hardware edits lock");
        hw.values.insert(id, value);
        hw.last_edit = Some(Instant::now());
    }
    if let Some(tx) = PARAM_CHANGE_NOTIFIER.lock().expect("param change notifier lock").as_ref() {
        let _ = tx.try_send((id, value));
    }
}

/// Preset/program changes are applied asynchronously — queue several refreshes.
fn notify_param_refresh_burst() {
    if let Some(tx) = PARAM_REFRESH_NOTIFIER.lock().expect("param refresh notifier lock").as_ref() {
        for _ in 0..12 {
            let _ = tx.try_send(());
        }
    }
}

/// Host component handler — Overbridge calls `requestOpenEditor` when it needs
/// the plugin UI attached before `deviceConnectionRequest` can succeed.
#[derive(Default)]
pub struct HostComponentHandler;

impl Class for HostComponentHandler {
    type Interfaces = (IComponentHandler, IComponentHandler2, IUnitHandler, IUnitHandler2);
}

impl IComponentHandlerTrait for HostComponentHandler {
    unsafe fn beginEdit(&self, id: ParamID) -> tresult {
        log_vst_handler!("IComponentHandler", "beginEdit", param_id = id);
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value_normalized: ParamValue) -> tresult {
        log_vst_handler!(
            "IComponentHandler",
            "performEdit",
            param_id = id,
            value = value_normalized
        );
        notify_param_change(id, value_normalized);
        kResultOk
    }

    unsafe fn endEdit(&self, id: ParamID) -> tresult {
        log_vst_handler!("IComponentHandler", "endEdit", param_id = id);
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: i32) -> tresult {
        let decoded = decode_restart_flags(flags);
        log_vst_handler!(
            "IComponentHandler",
            "restartComponent",
            flags = flags,
            flags_decoded = decoded.as_str()
        );
        let values_changed = flags & RestartFlags_::kParamValuesChanged != 0;
        let reload = flags & RestartFlags_::kReloadComponent != 0;
        if values_changed || reload {
            notify_param_refresh_burst();
        }
        kResultOk
    }
}

impl IComponentHandler2Trait for HostComponentHandler {
    unsafe fn setDirty(&self, state: u8) -> tresult {
        log_vst_handler!("IComponentHandler2", "setDirty", state = state);
        kResultOk
    }

    unsafe fn requestOpenEditor(&self, name: FIDString) -> tresult {
        let view = decode_fid_string(name);
        log_vst_handler!("IComponentHandler2", "requestOpenEditor", view = view.as_str());
        notify_editor_open_request();
        kResultOk
    }

    unsafe fn startGroupEdit(&self) -> tresult {
        log_vst_handler!("IComponentHandler2", "startGroupEdit");
        kResultOk
    }

    unsafe fn finishGroupEdit(&self) -> tresult {
        log_vst_handler!("IComponentHandler2", "finishGroupEdit");
        notify_param_refresh_burst();
        kResultOk
    }
}

impl IUnitHandlerTrait for HostComponentHandler {
    unsafe fn notifyUnitSelection(&self, unit_id: UnitID) -> tresult {
        log_vst_handler!("IUnitHandler", "notifyUnitSelection", unit_id = unit_id);
        notify_param_refresh_burst();
        kResultOk
    }

    unsafe fn notifyProgramListChange(&self, list_id: ProgramListID, program_index: i32) -> tresult {
        log_vst_handler!(
            "IUnitHandler",
            "notifyProgramListChange",
            list_id = list_id,
            program_index = program_index
        );
        notify_param_refresh_burst();
        kResultOk
    }
}

impl IUnitHandler2Trait for HostComponentHandler {
    unsafe fn notifyUnitByBusChange(&self) -> tresult {
        log_vst_handler!("IUnitHandler2", "notifyUnitByBusChange");
        notify_param_refresh_burst();
        kResultOk
    }
}

/// Minimal host application passed to `IPluginBase::initialize`.
#[derive(Default)]
pub struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        log_vst_handler!("IHostApplication", "getName", name_is_null = name.is_null());
        if name.is_null() {
            return kNotImplemented;
        }
        // UTF-16 "Overbridge Host" into String128 (128 TChar slots).
        const HOST_NAME: &[u16] = &[
            b'O' as u16, b'v' as u16, b'e' as u16, b'r' as u16, b'b' as u16, b'r' as u16,
            b'i' as u16, b'd' as u16, b'g' as u16, b'e' as u16, b' ' as u16, b'H' as u16,
            b'o' as u16, b's' as u16, b't' as u16, 0,
        ];
        let out = unsafe { &mut *name };
        for (i, ch) in HOST_NAME.iter().enumerate() {
            if i >= out.len() {
                break;
            }
            out[i] = *ch;
        }
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut std::ffi::c_void,
    ) -> tresult {
        log_vst_handler!(
            "IHostApplication",
            "createInstance",
            cid_is_null = cid.is_null(),
            iid_is_null = iid.is_null(),
            obj_is_null = obj.is_null()
        );
        if !obj.is_null() {
            unsafe { *obj = ptr::null_mut() };
        }
        kNotImplemented
    }
}

/// One automation point queue for a single parameter ID.
pub struct ParamValueQueueImpl {
    param_id: ParamID,
    points: RefCell<Vec<(i32, ParamValue)>>,
}

impl ParamValueQueueImpl {
    pub fn new(param_id: ParamID, value: ParamValue) -> Self {
        Self {
            param_id,
            points: RefCell::new(vec![(0, value)]),
        }
    }
}

impl Class for ParamValueQueueImpl {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamValueQueueImpl {
    unsafe fn getParameterId(&self) -> ParamID {
        self.param_id
    }

    unsafe fn getPointCount(&self) -> i32 {
        i32::try_from(self.points.borrow().len()).unwrap_or(i32::MAX)
    }

    unsafe fn getPoint(
        &self,
        index: i32,
        sample_offset: *mut i32,
        value: *mut ParamValue,
    ) -> tresult {
        let points = self.points.borrow();
        let Some((off, val)) = points.get(index as usize) else {
            return kNotImplemented;
        };
        if !sample_offset.is_null() {
            unsafe { *sample_offset = *off };
        }
        if !value.is_null() {
            unsafe { *value = *val };
        }
        kResultOk
    }

    unsafe fn addPoint(
        &self,
        sample_offset: i32,
        value: ParamValue,
        index: *mut i32,
    ) -> tresult {
        let mut points = self.points.borrow_mut();
        points.push((sample_offset, value));
        if !index.is_null() {
            unsafe {
                *index = i32::try_from(points.len().saturating_sub(1)).unwrap_or(i32::MAX);
            }
        }
        kResultOk
    }
}

/// Parameter change list handed to `ProcessData::inputParameterChanges`.
pub struct ParameterChangesList {
    queues: Vec<ComWrapper<ParamValueQueueImpl>>,
}

impl ParameterChangesList {
    pub fn from_changes(changes: Vec<(ParamID, ParamValue)>) -> Self {
        Self {
            queues: changes
                .into_iter()
                .map(|(id, value)| ComWrapper::new(ParamValueQueueImpl::new(id, value)))
                .collect(),
        }
    }
}

impl Class for ParameterChangesList {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParameterChangesList {
    unsafe fn getParameterCount(&self) -> i32 {
        i32::try_from(self.queues.len()).unwrap_or(i32::MAX)
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        self.queues
            .get(index as usize)
            .and_then(|q| q.to_com_ptr::<IParamValueQueue>())
            .map_or(ptr::null_mut(), |p| p.as_ptr())
    }

    unsafe fn addParameterData(
        &self,
        _id: *const ParamID,
        _index: *mut i32,
    ) -> *mut IParamValueQueue {
        ptr::null_mut()
    }
}
