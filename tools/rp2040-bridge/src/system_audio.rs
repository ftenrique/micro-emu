//! Operating-system level controls that do not require the emulated Codex Micro
//! device. `toggle_mic_mute` flips the default capture (microphone) endpoint
//! mute through the WASAPI device-enumerator COM API on Windows.

#[cfg(windows)]
mod native {
    use std::ffi::c_void;

    type Hresult = i32;
    type Ulong = u32;
    type Bool32 = i32; // Win32 BOOL

    const S_OK: Hresult = 0;
    const S_FALSE: Hresult = 1;
    const RPC_E_CHANGED_MODE: Hresult = 0x8000_0106u32 as i32;

    const COINIT_MULTITHREADED: Ulong = 0x0;
    const CLSCTX_ALL: Ulong = 0x17;

    // EDataFlow: eRender=0, eCapture=1, eAll=2.
    const E_CAPTURE: i32 = 1;
    // ERole: eConsole=0, eMultimedia=1, eCommunications=2.
    const E_CONSOLE: i32 = 0;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    // CLSID_MMDeviceEnumerator {BCDE0395-E52F-467C-8E3D-C4579291692E}
    const CLSID_MM_DEVICE_ENUMERATOR: Guid = Guid {
        data1: 0xBCDE_0395,
        data2: 0xE52F,
        data3: 0x467C,
        data4: [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
    };

    // IID_IMMDeviceEnumerator {A95664D2-9614-4F35-A746-DE8DB63617E6}
    const IID_IMM_DEVICE_ENUMERATOR: Guid = Guid {
        data1: 0xA956_64D2,
        data2: 0x9614,
        data3: 0x4F35,
        data4: [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
    };

    // IID_IAudioEndpointVolume {5CDF2C82-841E-4546-9722-0CF7407829A0}
    const IID_IAUDIO_ENDPOINT_VOLUME: Guid = Guid {
        data1: 0x5CDF_2C82,
        data2: 0x841E,
        data3: 0x4546,
        data4: [0x97, 0x22, 0x0C, 0xF7, 0x40, 0x78, 0x29, 0xA0],
    };

    type ReleaseFn = unsafe extern "system" fn(this: *mut c_void) -> Ulong;

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *const c_void, co_init: Ulong) -> Hresult;
        fn CoUninitialize();
        fn CoCreateInstance(
            clsid: *const Guid,
            outer: *mut c_void,
            cls_context: Ulong,
            iid: *const Guid,
            out: *mut *mut c_void,
        ) -> Hresult;
    }

    // COM vtables: IUnknown slots (QueryInterface, AddRef, Release) first, then
    // interface methods in documented order. Unused slots are pointer-sized
    // holes we never call, so their type is irrelevant to layout. The lpVtbl
    // field name mirrors the canonical COM-in-Rust spelling.

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ImmDeviceEnumeratorVtbl {
        _query_interface: *mut c_void,
        _add_ref: *mut c_void,
        release: ReleaseFn,
        _enum_audio_endpoints: *mut c_void,
        get_default_audio_endpoint: unsafe extern "system" fn(
            this: *mut c_void,
            data_flow: i32,
            role: i32,
            endpoint: *mut *mut c_void,
        ) -> Hresult,
        _get_device: *mut c_void,
        _register_callback: *mut c_void,
        _unregister_callback: *mut c_void,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ImmDeviceEnumerator {
        lpVtbl: *const ImmDeviceEnumeratorVtbl,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ImmDeviceVtbl {
        _query_interface: *mut c_void,
        _add_ref: *mut c_void,
        release: ReleaseFn,
        activate: unsafe extern "system" fn(
            this: *mut c_void,
            iid: *const Guid,
            cls_context: Ulong,
            activation_params: *mut c_void,
            interface_out: *mut *mut c_void,
        ) -> Hresult,
        _open_property_store: *mut c_void,
        _get_id: *mut c_void,
        _get_state: *mut c_void,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ImmDevice {
        lpVtbl: *const ImmDeviceVtbl,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct AudioEndpointVolumeVtbl {
        _query_interface: *mut c_void,
        _add_ref: *mut c_void,
        release: ReleaseFn,
        _register_notify: *mut c_void,
        _unregister_notify: *mut c_void,
        _get_channel_count: *mut c_void,
        _set_master_volume_level: *mut c_void,
        _set_master_volume_level_scalar: *mut c_void,
        _get_master_volume_level: *mut c_void,
        _get_master_volume_level_scalar: *mut c_void,
        _set_channel_volume_level: *mut c_void,
        _set_channel_volume_level_scalar: *mut c_void,
        _get_channel_volume_level: *mut c_void,
        _get_channel_volume_level_scalar: *mut c_void,
        set_mute:
            unsafe extern "system" fn(this: *mut c_void, mute: Bool32, event_context: *const Guid) -> Hresult,
        get_mute: unsafe extern "system" fn(this: *mut c_void, mute_out: *mut Bool32) -> Hresult,
        _get_volume_step_info: *mut c_void,
        _query_hardware_support: *mut c_void,
        _get_volume_range: *mut c_void,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct AudioEndpointVolume {
        lpVtbl: *const AudioEndpointVolumeVtbl,
    }

    fn succeeded(hr: Hresult) -> bool {
        hr >= 0
    }

    /// Toggles the mute state of the default microphone endpoint (console role).
    pub fn toggle_mic_mute() -> Result<(), String> {
        // COM is reference counted per thread: initialize for this call and
        // release afterward so the bridge thread is left as we found it.
        unsafe {
            let init = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED);
            if init == RPC_E_CHANGED_MODE {
                return Err("COM apartment mismatch on bridge thread".to_owned());
            }
            // S_OK (fresh) and S_FALSE (already initialized) are both usable.
            let need_uninit = init == S_OK || init == S_FALSE;
            let result = toggle_default_capture_mute();
            if need_uninit {
                CoUninitialize();
            }
            result
        }
    }

    fn toggle_default_capture_mute() -> Result<(), String> {
        unsafe {
            let mut enumerator_ptr: *mut c_void = std::ptr::null_mut();
            if !succeeded(CoCreateInstance(
                &CLSID_MM_DEVICE_ENUMERATOR,
                std::ptr::null_mut(),
                CLSCTX_ALL,
                &IID_IMM_DEVICE_ENUMERATOR,
                &mut enumerator_ptr,
            )) || enumerator_ptr.is_null()
            {
                return Err("Could not create the audio device enumerator".to_owned());
            }
            let enumerator = enumerator_ptr as *mut ImmDeviceEnumerator;

            let mut device_ptr: *mut c_void = std::ptr::null_mut();
            let device_hr = ((*(*enumerator).lpVtbl).get_default_audio_endpoint)(
                enumerator_ptr,
                E_CAPTURE,
                E_CONSOLE,
                &mut device_ptr,
            );
            if !succeeded(device_hr) || device_ptr.is_null() {
                release(enumerator_ptr, (*(*enumerator).lpVtbl).release);
                return Err(format!("No default microphone endpoint (hr=0x{device_hr:08X})"));
            }
            let device = device_ptr as *mut ImmDevice;

            let mut volume_ptr: *mut c_void = std::ptr::null_mut();
            let volume_hr = ((*(*device).lpVtbl).activate)(
                device_ptr,
                &IID_IAUDIO_ENDPOINT_VOLUME,
                CLSCTX_ALL,
                std::ptr::null_mut(),
                &mut volume_ptr,
            );
            if !succeeded(volume_hr) || volume_ptr.is_null() {
                release(device_ptr, (*(*device).lpVtbl).release);
                release(enumerator_ptr, (*(*enumerator).lpVtbl).release);
                return Err(format!("Could not activate endpoint volume (hr=0x{volume_hr:08X})"));
            }
            let volume = volume_ptr as *mut AudioEndpointVolume;

            let outcome = flip_mute(volume);

            release(volume_ptr, (*(*volume).lpVtbl).release);
            release(device_ptr, (*(*device).lpVtbl).release);
            release(enumerator_ptr, (*(*enumerator).lpVtbl).release);
            outcome
        }
    }

    fn flip_mute(volume: *mut AudioEndpointVolume) -> Result<(), String> {
        unsafe {
            let mut current: Bool32 = 0;
            let get_hr = ((*(*volume).lpVtbl).get_mute)(volume as *mut c_void, &mut current);
            if !succeeded(get_hr) {
                return Err(format!("GetMute failed (hr=0x{get_hr:08X})"));
            }
            let next: Bool32 = if current == 0 { 1 } else { 0 };
            let set_hr = ((*(*volume).lpVtbl).set_mute)(volume as *mut c_void, next, std::ptr::null());
            if !succeeded(set_hr) {
                return Err(format!("SetMute failed (hr=0x{set_hr:08X})"));
            }
            Ok(())
        }
    }

    fn release(this: *mut c_void, release_fn: ReleaseFn) {
        unsafe {
            if !this.is_null() {
                release_fn(this);
            }
        }
    }
}

#[cfg(windows)]
pub use native::toggle_mic_mute;

#[cfg(not(windows))]
pub fn toggle_mic_mute() -> Result<(), String> {
    Err("Microphone control is only available on Windows".to_owned())
}
