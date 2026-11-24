//! Electron's `powerMonitor`, as far as battery power: Chromium's `PowerMonitorDeviceSource` per
//! platform. Linux has none, so there it is always external power.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::App;

/// What the power source says where a test stands in for it.
#[cfg(feature = "parity")]
pub struct ScriptedPower {
    pub on_battery: bool,
}

#[cfg(feature = "parity")]
impl gpui_kit::Global for ScriptedPower {}

/// `None` is Chromium's `kUnknown`.
fn battery_power(cx: &App) -> Option<bool> {
    #[cfg(feature = "parity")]
    if let Some(scripted) = cx.try_global::<ScriptedPower>() {
        return Some(scripted.on_battery);
    }
    let _ = cx;
    source::battery_power()
}

/// `powerMonitor.isOnBatteryPower()`.
pub fn is_on_battery_power(cx: &App) -> bool {
    battery_power(cx) == Some(true)
}

/// `powerMonitor`'s `on-battery` (true) and `on-ac` (false) events.
pub fn on_battery_power_change(cx: &mut App, listener: impl Fn(bool, &mut App) + 'static) {
    let last = Cell::new(battery_power(cx));
    let changed = Rc::new(move |cx: &mut App| {
        let now = battery_power(cx);
        if last.replace(now) != now
            && let Some(on_battery) = now
        {
            listener(on_battery, cx);
        }
    });
    #[cfg(feature = "parity")]
    {
        let changed = changed.clone();
        cx.observe_global::<ScriptedPower>(move |cx| changed(cx))
            .detach();
    }
    let (send, changes) = async_channel::bounded(1);
    if !source::notify(send) {
        return;
    }
    cx.spawn(async move |cx| {
        while changes.recv().await.is_ok() {
            cx.update(|cx| changed(cx));
        }
    })
    .detach();
}

#[cfg(target_os = "macos")]
pub(super) mod source {
    use std::ffi::{c_char, c_void};
    use std::ptr;

    use objc2_core_foundation::{
        CFBoolean, CFDictionary, CFMutableDictionary, CFRetained, CFString, CFType,
    };

    #[repr(C)]
    struct Port(c_void);

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut CFMutableDictionary;
        fn IOServiceGetMatchingService(port: u32, matching: *mut CFMutableDictionary) -> u32;
        fn IORegistryEntryCreateCFProperties(
            entry: u32,
            properties: *mut *mut CFMutableDictionary,
            allocator: *const c_void,
            options: u32,
        ) -> i32;
        fn IOObjectRelease(object: u32) -> i32;
        fn IONotificationPortCreate(port: u32) -> *mut Port;
        fn IONotificationPortSetDispatchQueue(port: *mut Port, queue: *const c_void);
        fn IOServiceAddInterestNotification(
            port: *mut Port,
            service: u32,
            interest: *const c_char,
            callback: extern "C" fn(*mut c_void, u32, u32, *mut c_void),
            context: *mut c_void,
            notification: *mut u32,
        ) -> i32;
    }

    unsafe extern "C" {
        static _dispatch_main_q: c_void;
    }

    fn service() -> u32 {
        // SAFETY: a nul-terminated name, and the call consumes the matching dictionary.
        unsafe { IOServiceGetMatchingService(0, IOServiceMatching(c"IOPMPowerSource".as_ptr())) }
    }

    fn properties(service: u32) -> Option<CFRetained<CFDictionary<CFString, CFType>>> {
        let mut out = ptr::null_mut();
        // SAFETY: `out` is a valid out-pointer, and the caller owns what it gets.
        let result =
            unsafe { IORegistryEntryCreateCFProperties(service, &mut out, ptr::null(), 0) };
        let out = ptr::NonNull::new(out).filter(|_| result == 0)?;
        // SAFETY: the registry's properties are keyed by strings.
        Some(unsafe { CFRetained::from_raw(out.cast()) })
    }

    fn state(properties: &CFDictionary<CFString, CFType>) -> Option<bool> {
        let boolean = |key: &'static str| {
            let value = properties.get(&CFString::from_static_str(key))?;
            Some(value.downcast_ref::<CFBoolean>()?.as_bool())
        };
        if !boolean("BatteryInstalled")? {
            return Some(false);
        }
        Some(!boolean("ExternalConnected")?)
    }

    /// `BatteryLevelProviderMac`, except that Chromium also wants the battery's raw capacities.
    /// macOS 27 no longer has those at the top level, so there Chromium 142 never knows (#402).
    pub fn battery_power() -> Option<bool> {
        let service = service();
        if service == 0 {
            return Some(false);
        }
        let properties = properties(service);
        // SAFETY: the reference `service` returned, released once.
        unsafe { IOObjectRelease(service) };
        state(&*properties?)
    }

    extern "C" fn changed(context: *mut c_void, _: u32, _: u32, _: *mut c_void) {
        // SAFETY: the sender `notify` leaked for this callback.
        let send = unsafe { &*context.cast::<async_channel::Sender<()>>() };
        let _ = send.try_send(());
    }

    /// `IOPMPowerSourceSamplingEventSource`, for the life of the process.
    pub fn notify(send: async_channel::Sender<()>) -> bool {
        let service = service();
        if service == 0 {
            return false;
        }
        let context = Box::into_raw(Box::new(send));
        let mut notification = 0;
        // SAFETY: the port is checked, and the callback's context is never freed.
        unsafe {
            let port = IONotificationPortCreate(0);
            if port.is_null() {
                return false;
            }
            IONotificationPortSetDispatchQueue(port, &raw const _dispatch_main_q);
            IOServiceAddInterestNotification(
                port,
                service,
                c"IOGeneralInterest".as_ptr(),
                changed,
                context.cast(),
                &mut notification,
            ) == 0
        }
    }
}

#[cfg(windows)]
pub(super) mod source {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);

    #[repr(C)]
    struct Subscribe {
        callback: extern "system" fn(*mut c_void, u32, *mut c_void) -> u32,
        context: *mut c_void,
    }

    const ACDC_POWER_SOURCE: Guid = Guid(
        0x5d3e_9a59,
        0xe9d5,
        0x4b00,
        [0xa6, 0xbd, 0xff, 0x34, 0xff, 0x51, 0x65, 0x48],
    );
    const DEVICE_NOTIFY_CALLBACK: u32 = 2;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }

    #[link(name = "powrprof")]
    unsafe extern "system" {
        fn PowerSettingRegisterNotification(
            setting: *const Guid,
            flags: u32,
            recipient: *const Subscribe,
            handle: *mut *mut c_void,
        ) -> u32;
    }

    pub fn battery_power() -> Option<bool> {
        let mut status = SystemPowerStatus::default();
        // SAFETY: a valid out-pointer.
        let read = unsafe { GetSystemPowerStatus(&mut status) } != 0;
        read.then_some(status.ac_line_status == 0)
    }

    extern "system" fn changed(context: *mut c_void, _: u32, _: *mut c_void) -> u32 {
        // SAFETY: the sender `notify` leaked for this callback.
        let send = unsafe { &*context.cast::<async_channel::Sender<()>>() };
        let _ = send.try_send(());
        0
    }

    /// Upstream's `WM_POWERBROADCAST` message window, without a window: for the life of the
    /// process.
    pub fn notify(send: async_channel::Sender<()>) -> bool {
        let recipient = Box::leak(Box::new(Subscribe {
            callback: changed,
            context: Box::into_raw(Box::new(send)).cast(),
        }));
        let mut handle = std::ptr::null_mut();
        // SAFETY: the recipient and the callback's context are never freed.
        let result = unsafe {
            PowerSettingRegisterNotification(
                &ACDC_POWER_SOURCE,
                DEVICE_NOTIFY_CALLBACK,
                recipient,
                &mut handle,
            )
        };
        result == 0
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(super) mod source {
    pub fn battery_power() -> Option<bool> {
        Some(false)
    }

    pub fn notify(_: async_channel::Sender<()>) -> bool {
        false
    }
}
