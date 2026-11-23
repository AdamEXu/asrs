//! One tesseract.js worker: `tesseract-core-simd-lstm.wasm` and the parts of its Emscripten glue
//! and of `worker-script/index.js` that `createWorker("eng")` and `recognize` reach.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use asrs_js::number::to_int32;
use wasmtime::{
    AsContext, AsContextMut, Caller, Config, Engine, Extern, FuncType, Instance, Linker, Memory,
    Module, OptLevel, Ref, Store, Table, Trap, TypedFunc, Val,
};

const CORE_WASM: &[u8] = include_bytes!("../data/tesseract-core-simd-lstm.wasm");
const ENG_TRAINEDDATA_GZ: &[u8] = include_bytes!("../data/eng.traineddata.gz");

const OEM_LSTM_ONLY: i32 = 1;
const ENOENT: i32 = 44;
const EBADF: i32 = 8;
const EINVAL: i32 = 28;
const ENOTTY: i32 = 59;
const O_WRONLY: i32 = 1;

mod export {
    pub const MEMORY: &str = "Y";
    pub const TABLE: &str = "vf";
    pub const CALL_CTORS: &str = "Z";
    pub const NEW_TESS_BASE_API: &str = "Oc";
    pub const VERSION: &str = "Pc";
    pub const INIT_3: &str = "ed";
    pub const GET_PAGE_SEG_MODE: &str = "ld";
    pub const SET_IMAGE_FILE: &str = "qd";
    pub const SET_RECTANGLE: &str = "sd";
    pub const RECOGNIZE: &str = "Id";
    pub const GET_UTF8_TEXT: &str = "Md";
    pub const MEAN_TEXT_CONF: &str = "Td";
    pub const OEM: &str = "ce";
    pub const ERRNO_LOCATION: &str = "Bf";
    pub const MALLOC: &str = "Df";
    pub const STACK_SAVE: &str = "Ff";
    pub const STACK_RESTORE: &str = "Gf";
}

/// What tesseract.js rejects a job with: `err.toString()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OcrError(pub String);

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OcrError {}

#[derive(Debug)]
enum Thrown {
    Number(String),
    Error(String),
}

impl fmt::Display for Thrown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(text) | Self::Error(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for Thrown {}

fn abort(what: &str) -> wasmtime::Error {
    wasmtime::Error::from(Thrown::Error(format!(
        "RuntimeError: Aborted({what}). Build with -sASSERTIONS for more info."
    )))
}

fn to_ocr_error(error: &wasmtime::Error) -> OcrError {
    if let Some(thrown) = error.downcast_ref::<Thrown>() {
        return OcrError(thrown.to_string());
    }
    let message = match error.downcast_ref::<Trap>() {
        Some(Trap::MemoryOutOfBounds) => "memory access out of bounds".to_owned(),
        Some(Trap::TableOutOfBounds) => "table index is out of bounds".to_owned(),
        Some(Trap::IndirectCallToNull | Trap::BadSignature) => {
            "null function or function signature mismatch".to_owned()
        }
        Some(Trap::IntegerDivisionByZero) => "divide by zero".to_owned(),
        Some(Trap::IntegerOverflow) => "divide result unrepresentable".to_owned(),
        Some(Trap::UnreachableCodeReached) => "unreachable".to_owned(),
        Some(Trap::StackOverflow) => {
            return OcrError("RangeError: Maximum call stack size exceeded".to_owned());
        }
        _ => format!("{error:?}"),
    };
    OcrError(format!("RuntimeError: {message}"))
}

#[derive(Clone)]
enum Fd {
    Tty,
    File { path: String, position: u64 },
}

#[derive(Default)]
struct Host {
    files: BTreeMap<String, Vec<u8>>,
    fds: Vec<Option<Fd>>,
    memory: Option<Memory>,
    table: Option<Table>,
    stack_save: Option<TypedFunc<(), i32>>,
    stack_restore: Option<TypedFunc<i32, ()>>,
    errno_location: Option<TypedFunc<(), i32>>,
}

type HostCaller<'a> = Caller<'a, Host>;
type HostResult<T> = wasmtime::Result<T>;

fn absolute(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

fn memory(caller: &HostCaller<'_>) -> Memory {
    caller
        .data()
        .memory
        .expect("memory is set right after instantiation")
}

fn range(len: usize, at: u32, count: u32) -> HostResult<std::ops::Range<usize>> {
    let start = at as usize;
    let end = start + count as usize;
    if end > len {
        return Err(Trap::MemoryOutOfBounds.into());
    }
    Ok(start..end)
}

fn read_u32(caller: &HostCaller<'_>, at: u32) -> HostResult<u32> {
    let data = memory(caller).data(caller);
    let range = range(data.len(), at, 4)?;
    Ok(u32::from_le_bytes(data[range].try_into().expect("4 bytes")))
}

fn write_u32(caller: &mut HostCaller<'_>, at: u32, value: u32) -> HostResult<()> {
    write_bytes(caller, at, &value.to_le_bytes())
}

fn write_bytes(caller: &mut HostCaller<'_>, at: u32, bytes: &[u8]) -> HostResult<()> {
    let data = memory(caller).data_mut(caller);
    let range = range(data.len(), at, bytes.len() as u32)?;
    data[range].copy_from_slice(bytes);
    Ok(())
}

fn c_string(data: &[u8], at: u32) -> String {
    if at == 0 {
        return String::new();
    }
    let bytes = data.get(at as usize..).unwrap_or_default();
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn read_string(caller: &HostCaller<'_>, at: u32) -> String {
    c_string(memory(caller).data(caller), at)
}

fn load_embedded_files(caller: &mut HostCaller<'_>, mut at: u32) -> HostResult<()> {
    loop {
        let name = read_u32(caller, at)?;
        let len = read_u32(caller, at + 4)?;
        let content = read_u32(caller, at + 8)?;
        at += 12;
        let path = absolute(&read_string(caller, name));
        let data = memory(caller).data(&mut *caller);
        let bytes = data[range(data.len(), content, len)?].to_vec();
        caller.data_mut().files.insert(path, bytes);
        if read_u32(caller, at)? == 0 {
            return Ok(());
        }
    }
}

fn open(caller: &mut HostCaller<'_>, path: u32, flags: i32) -> i32 {
    let path = read_string(caller, path);
    if path.is_empty() {
        return -ENOENT;
    }
    let path = absolute(&path);
    let host = caller.data_mut();
    if flags & 64 != 0 {
        host.files.entry(path.clone()).or_default();
    }
    if !host.files.contains_key(&path) {
        return -ENOENT;
    }
    if flags & 512 != 0 {
        host.files.insert(path.clone(), Vec::new());
    }
    let fd = Some(Fd::File { path, position: 0 });
    let slot = host.fds.iter().position(Option::is_none);
    let slot = match slot {
        Some(slot) => {
            host.fds[slot] = fd;
            slot
        }
        None => {
            host.fds.push(fd);
            host.fds.len() - 1
        }
    };
    slot as i32
}

fn close(caller: &mut HostCaller<'_>, fd: i32) -> i32 {
    match caller.data_mut().fds.get_mut(fd as usize) {
        Some(slot @ Some(_)) => {
            *slot = None;
            0
        }
        _ => EBADF,
    }
}

fn fd(caller: &HostCaller<'_>, fd: i32) -> Option<Fd> {
    caller.data().fds.get(fd as usize).cloned().flatten()
}

fn set_position(caller: &mut HostCaller<'_>, fd: i32, to: u64) {
    if let Some(Some(Fd::File { position, .. })) = caller.data_mut().fds.get_mut(fd as usize) {
        *position = to;
    }
}

fn io_vectors(caller: &HostCaller<'_>, iov: u32, count: u32) -> HostResult<Vec<(u32, u32)>> {
    (0..count)
        .map(|i| {
            Ok((
                read_u32(caller, iov + i * 8)?,
                read_u32(caller, iov + i * 8 + 4)?,
            ))
        })
        .collect()
}

fn fd_read(
    caller: &mut HostCaller<'_>,
    handle: i32,
    iov: u32,
    count: u32,
    result: u32,
) -> HostResult<i32> {
    let Some(entry) = fd(caller, handle) else {
        return Ok(EBADF);
    };
    let Fd::File { path, position } = entry else {
        return Ok(6);
    };
    let contents = caller.data().files[&path].clone();
    let mut at = (position as usize).min(contents.len());
    let mut total = 0;
    for (pointer, len) in io_vectors(caller, iov, count)? {
        let read = (len as usize).min(contents.len() - at);
        write_bytes(caller, pointer, &contents[at..at + read])?;
        at += read;
        total += read as u32;
        if read < len as usize {
            break;
        }
    }
    set_position(caller, handle, position + u64::from(total));
    write_u32(caller, result, total)?;
    Ok(0)
}

fn fd_write(
    caller: &mut HostCaller<'_>,
    handle: i32,
    iov: u32,
    count: u32,
    result: u32,
) -> HostResult<i32> {
    let Some(entry) = fd(caller, handle) else {
        return Ok(EBADF);
    };
    let mut bytes = Vec::new();
    for (pointer, len) in io_vectors(caller, iov, count)? {
        let data = memory(caller).data(&*caller);
        bytes.extend_from_slice(&data[range(data.len(), pointer, len)?]);
    }
    if let Fd::File { path, position } = entry {
        let file = caller.data_mut().files.entry(path).or_default();
        let start = position as usize;
        if file.len() < start + bytes.len() {
            file.resize(start + bytes.len(), 0);
        }
        file[start..start + bytes.len()].copy_from_slice(&bytes);
        set_position(caller, handle, position + bytes.len() as u64);
    }
    write_u32(caller, result, bytes.len() as u32)?;
    Ok(0)
}

fn fd_seek(
    caller: &mut HostCaller<'_>,
    handle: i32,
    low: i32,
    high: i32,
    whence: i32,
    result: u32,
) -> HostResult<i32> {
    let offset = (i64::from(high) << 32) | i64::from(low as u32);
    let Some(entry) = fd(caller, handle) else {
        return Ok(EBADF);
    };
    let Fd::File { path, position } = entry else {
        return Ok(70);
    };
    let base = match whence {
        0 => 0,
        1 => position as i64,
        2 => caller.data().files[&path].len() as i64,
        _ => return Ok(EINVAL),
    };
    let target = base + offset;
    if target < 0 {
        return Ok(EINVAL);
    }
    set_position(caller, handle, target as u64);
    write_bytes(caller, result, &target.to_le_bytes())?;
    Ok(0)
}

fn fcntl(caller: &mut HostCaller<'_>, handle: i32, command: i32) -> HostResult<i32> {
    if fd(caller, handle).is_none() {
        return Ok(-EBADF);
    }
    match command {
        1 | 2 | 6 | 7 => Ok(0),
        3 => Ok(0),
        8 | 16 => Ok(-EINVAL),
        _ => Err(wasmtime::Error::from(Thrown::Error(format!(
            "unported fcntl {command}"
        )))),
    }
}

fn resize_heap(caller: &mut HostCaller<'_>, requested: u32) -> bool {
    const MAX: f64 = 2_147_483_648.0;
    let memory = memory(caller);
    let old = memory.data_size(&*caller) as f64;
    let requested = f64::from(requested);
    if requested > MAX {
        return false;
    }
    for cut_down in [1.0, 2.0, 4.0] {
        let over = (old * (1.0 + 0.2 / cut_down)).min(requested + 100_663_296.0);
        let target = requested.max(over);
        let target = MAX.min(target + (65_536.0 - target % 65_536.0) % 65_536.0);
        let pages = to_int32((target - old + 65_535.0) / 65_536.0);
        if memory.grow(&mut *caller, pages as u64).is_ok() {
            return true;
        }
    }
    false
}

const ENVIRONMENT: [&str; 7] = [
    "USER=web_user",
    "LOGNAME=web_user",
    "PATH=/",
    "PWD=/",
    "HOME=/home/web_user",
    "LANG=en_US.UTF-8",
    "_=./this.program",
];

fn environ_get(caller: &mut HostCaller<'_>, pointers: u32, buffer: u32) -> HostResult<()> {
    let mut offset = 0;
    for (index, entry) in ENVIRONMENT.iter().enumerate() {
        write_u32(caller, pointers + 4 * index as u32, buffer + offset)?;
        write_bytes(caller, buffer + offset, entry.as_bytes())?;
        write_bytes(caller, buffer + offset + entry.len() as u32, &[0])?;
        offset += entry.len() as u32 + 1;
    }
    Ok(())
}

fn invoke(
    caller: &mut HostCaller<'_>,
    args: &[Val],
    ty: &FuncType,
    results: &mut [Val],
) -> HostResult<()> {
    let host = caller.data();
    let (save, restore, table) = (
        host.stack_save.clone().expect("exports are resolved"),
        host.stack_restore.clone().expect("exports are resolved"),
        host.table.expect("exports are resolved"),
    );
    let stack = save.call(&mut *caller, ())?;
    let index = args[0].unwrap_i32() as u32;
    let callee = match table.get(&mut *caller, u64::from(index)) {
        Some(Ref::Func(Some(callee))) => callee,
        _ => {
            restore.call(&mut *caller, stack)?;
            return Err(Thrown::Error(format!(
                "TypeError: invoke of table entry {index}"
            ))
            .into());
        }
    };
    debug_assert_eq!(ty.results().len(), results.len());
    match callee.call(&mut *caller, &args[1..], results) {
        Ok(()) => Ok(()),
        Err(error) => {
            restore.call(&mut *caller, stack)?;
            match error.downcast_ref::<Thrown>() {
                Some(Thrown::Number(_)) => Err(abort("-1")),
                _ => Err(error),
            }
        }
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

fn int(args: &[Val], index: usize) -> i32 {
    args[index].unwrap_i32()
}

fn uint(args: &[Val], index: usize) -> u32 {
    int(args, index) as u32
}

fn call_import(
    name: &str,
    ty: &FuncType,
    caller: &mut HostCaller<'_>,
    args: &[Val],
    results: &mut [Val],
) -> HostResult<()> {
    let mut answer = |value: i32| results[0] = Val::I32(value);
    match name {
        "b" | "c" | "d" | "e" | "f" | "g" | "h" | "i" | "j" | "s" | "t" => {
            invoke(caller, args, ty, results)?;
        }
        "a" => {
            let [condition, file, function] =
                [0, 1, 3].map(|i| (uint(args, i) != 0).then(|| read_string(caller, uint(args, i))));
            return Err(abort(&format!(
                "Assertion failed: {}, at: {},{},{}",
                condition.unwrap_or_default(),
                file.unwrap_or_else(|| "unknown filename".to_owned()),
                int(args, 2),
                function.unwrap_or_else(|| "unknown function".to_owned()),
            )));
        }
        "k" => return Err(abort("")),
        "l" => return Err(Thrown::Number(uint(args, 0).to_string()).into()),
        "x" => return Err(Thrown::Number("Infinity".to_owned()).into()),
        "F" | "P" | "U" | "X" => return Err(abort("-1")),
        "m" => results[0] = Val::F64(now_ms().to_bits()),
        "O" => results[0] = Val::F64(now_ms().to_bits()),
        "n" => answer(fd_write(
            caller,
            int(args, 0),
            uint(args, 1),
            uint(args, 2),
            uint(args, 3),
        )?),
        "o" => answer(close(caller, int(args, 0))),
        "p" => answer(open(caller, uint(args, 1), int(args, 2))),
        "q" => answer(fd_read(
            caller,
            int(args, 0),
            uint(args, 1),
            uint(args, 2),
            uint(args, 3),
        )?),
        "r" => answer(fcntl(caller, int(args, 0), int(args, 1))?),
        "u" => {
            if let Some(result) = results.first_mut() {
                *result = Val::I32(0);
            }
        }
        "v" => answer(fd_seek(
            caller,
            int(args, 0),
            int(args, 1),
            int(args, 2),
            int(args, 3),
            uint(args, 4),
        )?),
        "y" => answer(i32::from(resize_heap(caller, uint(args, 0)))),
        "I" => {
            environ_get(caller, uint(args, 0), uint(args, 1))?;
            answer(0);
        }
        "J" => {
            write_u32(caller, uint(args, 0), ENVIRONMENT.len() as u32)?;
            let size: usize = ENVIRONMENT.iter().map(|entry| entry.len() + 1).sum();
            write_u32(caller, uint(args, 1), size as u32)?;
            answer(0);
        }
        "R" => {
            let [to, from, count] = [0, 1, 2].map(|i| uint(args, i));
            let data = memory(caller).data_mut(&mut *caller);
            let source = range(data.len(), from, count)?;
            range(data.len(), to, count)?;
            data.copy_within(source, to as usize);
        }
        "S" => match fd(caller, int(args, 0)) {
            None => answer(-EBADF),
            Some(Fd::Tty) => answer(0),
            Some(Fd::File { .. }) => answer(-ENOTTY),
        },
        "T" => load_embedded_files(caller, uint(args, 0))?,
        _ => {
            return Err(Thrown::Error(format!("unported import {name}")).into());
        }
    }
    Ok(())
}

struct Core {
    engine: Engine,
    module: Module,
    traineddata: Vec<u8>,
}

impl Core {
    fn load() -> Result<Self, OcrError> {
        let mut config = Config::new();
        config.cranelift_opt_level(OptLevel::Speed);
        config.max_wasm_stack(1 << 20);
        let engine = Engine::new(&config).map_err(|e| to_ocr_error(&e))?;
        let module = Module::new(&engine, CORE_WASM).map_err(|e| to_ocr_error(&e))?;
        Ok(Self {
            engine,
            module,
            traineddata: gunzip(ENG_TRAINEDDATA_GZ)?,
        })
    }

    fn shared() -> Result<&'static Arc<Self>, OcrError> {
        static CORE: OnceLock<Result<Arc<Core>, OcrError>> = OnceLock::new();
        CORE.get_or_init(|| Core::load().map(Arc::new))
            .as_ref()
            .map_err(Clone::clone)
    }
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>, OcrError> {
    let bad = || OcrError("Error: incorrect header check".to_owned());
    if data.len() < 18 || data[..3] != [0x1f, 0x8b, 8] {
        return Err(bad());
    }
    let flags = data[3];
    let mut at = 10;
    if flags & 4 != 0 {
        at += 2 + usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            at += 1 + data[at..].iter().position(|b| *b == 0).ok_or_else(bad)?;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    miniz_oxide::inflate::decompress_to_vec(&data[at..data.len() - 8]).map_err(|_| bad())
}

/// The EXIF orientation `setImage.js` finds in the first 500 bytes.
fn exif_orientation(image: &[u8]) -> i32 {
    const MARKER: [u8; 9] = [18, 0, 3, 0, 0, 0, 1, 0, 0];
    let head = &image[..image.len().min(500)];
    let text = head
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let _ = MARKER;
    let needle = "1 18 0 3 0 0 0 1 0 ";
    let mut from = 0;
    while let Some(found) = text[from..].find(needle) {
        let after = from + found + needle.len();
        if let Some(digit) = text[after..].chars().next().and_then(|c| c.to_digit(10)) {
            return if digit == 0 { 1 } else { digit as i32 };
        }
        from += found + 1;
    }
    1
}

/// A tesseract.js worker created by `createWorker("eng")`.
pub struct Worker {
    store: Store<Host>,
    memory: Memory,
    api: i32,
    set_image_file: TypedFunc<(i32, i32, f32), i32>,
    set_rectangle: TypedFunc<(i32, i32, i32, i32, i32), ()>,
    recognize: TypedFunc<(i32, i32), i32>,
    get_utf8_text: TypedFunc<i32, i32>,
    mean_text_conf: TypedFunc<i32, i32>,
    get_page_seg_mode: TypedFunc<i32, i32>,
    oem: TypedFunc<i32, i32>,
    version: TypedFunc<i32, i32>,
}

impl Worker {
    pub fn new() -> Result<Self, OcrError> {
        let core = Core::shared()?;
        Self::create(core).map_err(|e| to_ocr_error(&e))
    }

    fn create(core: &Core) -> wasmtime::Result<Self> {
        let mut linker: Linker<Host> = Linker::new(&core.engine);
        for import in core.module.imports() {
            let wasmtime::ExternType::Func(ty) = import.ty() else {
                return Err(abort("unexpected import"));
            };
            let name = import.name().to_owned();
            let signature = ty.clone();
            linker.func_new(
                import.module(),
                import.name(),
                ty,
                move |mut caller, args, results| {
                    call_import(&name, &signature, &mut caller, args, results)
                },
            )?;
        }
        let mut store = Store::new(&core.engine, Host::default());
        store.data_mut().fds = vec![Some(Fd::Tty), Some(Fd::Tty), Some(Fd::Tty)];
        let instance = linker.instantiate(&mut store, &core.module)?;
        let memory = instance
            .get_memory(&mut store, export::MEMORY)
            .ok_or_else(|| abort("no memory export"))?;
        let table = match instance.get_export(&mut store, export::TABLE) {
            Some(Extern::Table(table)) => table,
            _ => return Err(abort("no table export")),
        };
        let stack_save = instance.get_typed_func(&mut store, export::STACK_SAVE)?;
        let stack_restore = instance.get_typed_func(&mut store, export::STACK_RESTORE)?;
        let errno_location = instance.get_typed_func(&mut store, export::ERRNO_LOCATION)?;
        let host = store.data_mut();
        host.memory = Some(memory);
        host.table = Some(table);
        host.stack_save = Some(stack_save);
        host.stack_restore = Some(stack_restore);
        host.errno_location = Some(errno_location);

        let ctors: TypedFunc<(), ()> = instance.get_typed_func(&mut store, export::CALL_CTORS)?;
        ctors.call(&mut store, ())?;
        store
            .data_mut()
            .files
            .insert("/eng.traineddata".to_owned(), core.traineddata.clone());

        let new_api: TypedFunc<(), i32> =
            instance.get_typed_func(&mut store, export::NEW_TESS_BASE_API)?;
        let api = new_api.call(&mut store, ())?;
        Self::init(&mut store, &instance, memory, api)?;
        Ok(Self {
            set_image_file: instance.get_typed_func(&mut store, export::SET_IMAGE_FILE)?,
            set_rectangle: instance.get_typed_func(&mut store, export::SET_RECTANGLE)?,
            recognize: instance.get_typed_func(&mut store, export::RECOGNIZE)?,
            get_utf8_text: instance.get_typed_func(&mut store, export::GET_UTF8_TEXT)?,
            mean_text_conf: instance.get_typed_func(&mut store, export::MEAN_TEXT_CONF)?,
            get_page_seg_mode: instance.get_typed_func(&mut store, export::GET_PAGE_SEG_MODE)?,
            oem: instance.get_typed_func(&mut store, export::OEM)?,
            version: instance.get_typed_func(&mut store, export::VERSION)?,
            store,
            memory,
            api,
        })
    }

    fn init(
        store: &mut Store<Host>,
        instance: &Instance,
        memory: Memory,
        api: i32,
    ) -> wasmtime::Result<()> {
        let malloc: TypedFunc<i32, i32> = instance.get_typed_func(&mut *store, export::MALLOC)?;
        let buffer = malloc.call(&mut *store, 128)?;
        memory.write(store.as_context_mut(), buffer as usize, b"eng\0")?;
        let init: TypedFunc<(i32, i32, i32, i32), i32> =
            instance.get_typed_func(&mut *store, export::INIT_3)?;
        match init.call(&mut *store, (api, 0, buffer, OEM_LSTM_ONLY))? {
            -1 => Err(Thrown::Error("initialization failed".to_owned()).into()),
            _ => Ok(()),
        }
    }

    /// `recognize(image, { rectangle })` with `rectangle` as `[left, top, width, height]`;
    /// resolves to `data.text`.
    pub fn recognize(&mut self, image: &[u8], rectangle: [f64; 4]) -> Result<String, OcrError> {
        self.run(image, rectangle).map_err(|e| to_ocr_error(&e))
    }

    fn run(&mut self, image: &[u8], rectangle: [f64; 4]) -> wasmtime::Result<String> {
        let store = &mut self.store;
        let api = self.api;
        store
            .data_mut()
            .files
            .insert("/input".to_owned(), image.to_vec());
        let status = self
            .set_image_file
            .call(&mut *store, (api, exif_orientation(image), 0.0))?;
        if status == 1 {
            return Err(Thrown::Error("Error: Error attempting to read image.".to_owned()).into());
        }
        let [left, top, width, height] = rectangle.map(to_int32);
        self.set_rectangle
            .call(&mut *store, (api, left, top, width, height))?;
        self.recognize.call(&mut *store, (api, 0))?;
        let pointer = self.get_utf8_text.call(&mut *store, api)?;
        let text = c_string(self.memory.data(store.as_context()), pointer as u32);
        self.mean_text_conf.call(&mut *store, api)?;
        self.get_page_seg_mode.call(&mut *store, api)?;
        self.oem.call(&mut *store, api)?;
        self.version.call(&mut *store, api)?;
        Ok(text)
    }
}
