// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! qpdf, for what Poppler cannot do: rewriting a whole document — encrypted
//! with a password, with its permissions, or packed smaller.
//!
//! Only qpdf's plain C interface is used, declared here by hand, the way the
//! newer Poppler calls are in `newer`; the handful of calls needed do not
//! warrant generated bindings and the compiler those need. Everything is
//! wrapped in `Qpdf`, which owns one document and frees it when dropped, and
//! is used on one thread only.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::Path;

type Data = *mut c_void;
type Handle = c_uint;
type Bool = c_int;
type ErrorCode = c_int;

const QPDF_ERRORS: ErrorCode = 1 << 1;
/// `qpdf_e_password`, from qpdf's Constants.h.
const ERROR_PASSWORD: c_int = 4;

/// `qpdf_object_stream_e`
const OBJECT_STREAMS_GENERATE: c_int = 2;
/// `qpdf_stream_decode_level_e`
const DECODE_NONE: c_int = 0;
const DECODE_GENERALIZED: c_int = 1;
/// `qpdf_r3_print_e`
pub const PRINT_FULL: c_int = 0;
pub const PRINT_NONE: c_int = 2;

// Linked by build.rs, which finds qpdf through pkg-config.
extern "C" {
    fn qpdf_init() -> Data;
    fn qpdf_cleanup(qpdf: *mut Data);
    fn qpdf_silence_errors(qpdf: Data);
    fn qpdf_set_suppress_warnings(qpdf: Data, value: Bool);
    fn qpdf_has_error(qpdf: Data) -> Bool;
    fn qpdf_get_error(qpdf: Data) -> *mut c_void;
    fn qpdf_get_error_code(qpdf: Data, error: *mut c_void) -> c_int;
    fn qpdf_get_error_message_detail(qpdf: Data, error: *mut c_void) -> *const c_char;
    fn qpdf_read(qpdf: Data, filename: *const c_char, password: *const c_char) -> ErrorCode;
    fn qpdf_init_write(qpdf: Data, filename: *const c_char) -> ErrorCode;
    fn qpdf_set_object_stream_mode(qpdf: Data, mode: c_int);
    fn qpdf_set_compress_streams(qpdf: Data, value: Bool);
    fn qpdf_set_decode_level(qpdf: Data, level: c_int);
    fn qpdf_set_preserve_encryption(qpdf: Data, value: Bool);
    #[allow(clippy::too_many_arguments)]
    fn qpdf_set_r6_encryption_parameters2(
        qpdf: Data,
        user_password: *const c_char,
        owner_password: *const c_char,
        allow_accessibility: Bool,
        allow_extract: Bool,
        allow_assemble: Bool,
        allow_annotate_and_form: Bool,
        allow_form_filling: Bool,
        allow_modify_other: Bool,
        print: c_int,
        encrypt_metadata: Bool,
    );
    fn qpdf_write(qpdf: Data) -> ErrorCode;
    fn qpdf_allow_print_low_res(qpdf: Data) -> Bool;
    fn qpdf_allow_extract_all(qpdf: Data) -> Bool;
    fn qpdf_allow_modify_annotation(qpdf: Data) -> Bool;
    fn qpdf_allow_modify_assembly(qpdf: Data) -> Bool;

    fn qpdf_push_inherited_attributes_to_page(qpdf: Data) -> ErrorCode;
    fn qpdf_get_num_pages(qpdf: Data) -> c_int;
    fn qpdf_get_page_n(qpdf: Data, index: usize) -> Handle;
    fn qpdf_oh_release_all(qpdf: Data);
    fn qpdf_oh_is_initialized(qpdf: Data, oh: Handle) -> Bool;
    fn qpdf_oh_is_dictionary(qpdf: Data, oh: Handle) -> Bool;
    fn qpdf_oh_is_stream(qpdf: Data, oh: Handle) -> Bool;
    fn qpdf_oh_is_array(qpdf: Data, oh: Handle) -> Bool;
    fn qpdf_oh_is_name(qpdf: Data, oh: Handle) -> Bool;
    fn qpdf_oh_is_number(qpdf: Data, oh: Handle) -> Bool;
    fn qpdf_oh_is_bool(qpdf: Data, oh: Handle) -> Bool;
    fn qpdf_oh_get_bool_value(qpdf: Data, oh: Handle) -> Bool;
    fn qpdf_oh_get_numeric_value(qpdf: Data, oh: Handle) -> f64;
    fn qpdf_oh_get_int_value_as_int(qpdf: Data, oh: Handle) -> c_int;
    fn qpdf_oh_get_name(qpdf: Data, oh: Handle) -> *const c_char;
    fn qpdf_oh_get_array_n_items(qpdf: Data, oh: Handle) -> c_int;
    fn qpdf_oh_get_array_item(qpdf: Data, oh: Handle, n: c_int) -> Handle;
    fn qpdf_oh_has_key(qpdf: Data, oh: Handle, key: *const c_char) -> Bool;
    fn qpdf_oh_get_key(qpdf: Data, oh: Handle, key: *const c_char) -> Handle;
    fn qpdf_oh_begin_dict_key_iter(qpdf: Data, dict: Handle);
    fn qpdf_oh_dict_more_keys(qpdf: Data) -> Bool;
    fn qpdf_oh_dict_next_key(qpdf: Data) -> *const c_char;
    fn qpdf_oh_get_dict(qpdf: Data, oh: Handle) -> Handle;
    fn qpdf_oh_get_object_id(qpdf: Data, oh: Handle) -> c_int;
    fn qpdf_oh_get_generation(qpdf: Data, oh: Handle) -> c_int;
    fn qpdf_oh_get_stream_data(
        qpdf: Data,
        stream: Handle,
        decode_level: c_int,
        filtered: *mut Bool,
        data: *mut *mut u8,
        len: *mut usize,
    ) -> ErrorCode;
    fn qpdf_oh_replace_stream_data(
        qpdf: Data,
        stream: Handle,
        data: *const u8,
        len: usize,
        filter: Handle,
        decode_parms: Handle,
    );
    fn qpdf_oh_new_name(qpdf: Data, name: *const c_char) -> Handle;
    fn qpdf_oh_new_null(qpdf: Data) -> Handle;
    fn qpdf_oh_new_integer(qpdf: Data, value: i64) -> Handle;
    fn qpdf_oh_replace_key(qpdf: Data, oh: Handle, key: *const c_char, item: Handle);
    fn qpdf_oh_remove_key(qpdf: Data, oh: Handle, key: *const c_char);
    fn qpdf_oh_new_stream(qpdf: Data) -> Handle;
    fn qpdf_oh_new_dictionary(qpdf: Data) -> Handle;
    fn qpdf_oh_new_array(qpdf: Data) -> Handle;
    fn qpdf_oh_append_item(qpdf: Data, oh: Handle, item: Handle);
    fn qpdf_oh_new_real_from_double(qpdf: Data, value: f64, decimal_places: c_int) -> Handle;
    fn qpdf_make_indirect_object(qpdf: Data, oh: Handle) -> Handle;
    fn qpdf_get_root(qpdf: Data) -> Handle;
}

/// Why qpdf could not do something.
#[derive(Debug, PartialEq)]
pub enum Error {
    /// The password given did not open the document.
    Password,
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Password => f.write_str("the password is not right"),
            Error::Other(message) => f.write_str(message),
        }
    }
}

/// An object in the document, valid while the `Qpdf` it came from is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Object(Handle);

/// What the document's permissions allow, when it is written encrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub print: bool,
    pub copy: bool,
    pub annotate: bool,
    /// Filling in forms, and adding, removing and turning pages.
    pub change: bool,
}

/// How a document is written out.
pub enum Protection<'a> {
    /// As it came: the same password, if any.
    Keep,
    /// With no password and no restrictions.
    Remove,
    /// With these passwords: `open`, empty for none, and `owner`, which
    /// changes the permissions.
    Set { open: &'a str, owner: &'a str, allow: Permissions },
}

pub struct Qpdf {
    data: Data,
}

fn c_path(path: &Path) -> Result<CString, Error> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes()).map_err(|_| Error::Other("the file's name cannot be used".into()))
}

fn c_text(text: &str) -> Result<CString, Error> {
    CString::new(text).map_err(|_| Error::Other("a password cannot contain a zero byte".into()))
}

impl Qpdf {
    /// Read a document, with its password if it has one.
    pub fn read(path: &Path, password: Option<&str>) -> Result<Self, Error> {
        let filename = c_path(path)?;
        let password = c_text(password.unwrap_or(""))?;
        // SAFETY: a fresh handle, freed in Drop; the strings outlive the call.
        let qpdf = unsafe {
            let data = qpdf_init();
            qpdf_silence_errors(data);
            qpdf_set_suppress_warnings(data, 1);
            Qpdf { data }
        };
        let code = unsafe { qpdf_read(qpdf.data, filename.as_ptr(), password.as_ptr()) };
        qpdf.check(code)?;
        Ok(qpdf)
    }

    /// What the permissions allow — as written in the file, whichever
    /// password opened it.
    pub fn permissions(&self) -> Permissions {
        // SAFETY: a live handle.
        unsafe {
            Permissions {
                print: qpdf_allow_print_low_res(self.data) != 0,
                copy: qpdf_allow_extract_all(self.data) != 0,
                annotate: qpdf_allow_modify_annotation(self.data) != 0,
                change: qpdf_allow_modify_assembly(self.data) != 0,
            }
        }
    }

    /// Turn qpdf's report of a call into a result.
    fn check(&self, code: ErrorCode) -> Result<(), Error> {
        // SAFETY: a live handle; the error and its text belong to it.
        unsafe {
            if code & QPDF_ERRORS == 0 && qpdf_has_error(self.data) == 0 {
                return Ok(());
            }
            let error = qpdf_get_error(self.data);
            if error.is_null() {
                return Err(Error::Other("qpdf failed without saying why".into()));
            }
            if qpdf_get_error_code(self.data, error) == ERROR_PASSWORD {
                return Err(Error::Password);
            }
            let detail = qpdf_get_error_message_detail(self.data, error);
            let detail = if detail.is_null() {
                "qpdf failed without saying why".to_string()
            } else {
                CStr::from_ptr(detail).to_string_lossy().into_owned()
            };
            Err(Error::Other(detail))
        }
    }

    /// Write the document to `path`, packed as small as it goes without
    /// changing anything that shows, and protected as asked.
    pub fn write(&self, path: &Path, protection: Protection<'_>) -> Result<(), Error> {
        let filename = c_path(path)?;
        // SAFETY: a live handle; every string outlives the call it is in.
        unsafe {
            self.check(qpdf_init_write(self.data, filename.as_ptr()))?;
            qpdf_set_object_stream_mode(self.data, OBJECT_STREAMS_GENERATE);
            qpdf_set_compress_streams(self.data, 1);
            // Streams written loosely, or with an old filter, are unpacked
            // and packed again properly; images keep their own encoding.
            qpdf_set_decode_level(self.data, DECODE_GENERALIZED);
            match protection {
                Protection::Keep => {}
                Protection::Remove => qpdf_set_preserve_encryption(self.data, 0),
                Protection::Set { open, owner, allow } => {
                    let open = c_text(open)?;
                    let owner = c_text(owner)?;
                    let flag = |on: bool| Bool::from(on);
                    qpdf_set_r6_encryption_parameters2(
                        self.data,
                        open.as_ptr(),
                        owner.as_ptr(),
                        // Screen readers always, as the PDF standard asks.
                        1,
                        flag(allow.copy),
                        flag(allow.change),
                        flag(allow.annotate),
                        flag(allow.change),
                        flag(allow.change),
                        if allow.print { PRINT_FULL } else { PRINT_NONE },
                        1,
                    );
                }
            }
            self.check(qpdf_write(self.data))
        }
    }

    // --- Objects, for finding and replacing pictures. ---

    pub fn pages(&self) -> Vec<Object> {
        // SAFETY: a live handle.
        unsafe {
            if qpdf_push_inherited_attributes_to_page(self.data) & QPDF_ERRORS != 0 {
                return Vec::new();
            }
            let count = usize::try_from(qpdf_get_num_pages(self.data)).unwrap_or(0);
            (0..count).map(|i| Object(qpdf_get_page_n(self.data, i))).collect()
        }
    }

    /// A key of a dictionary, or of a stream's dictionary.
    pub fn get(&self, object: Object, key: &str) -> Option<Object> {
        let key = CString::new(format!("/{key}")).ok()?;
        // SAFETY: a live handle and object.
        unsafe {
            let dict = if qpdf_oh_is_stream(self.data, object.0) != 0 {
                qpdf_oh_get_dict(self.data, object.0)
            } else {
                object.0
            };
            if qpdf_oh_is_dictionary(self.data, dict) == 0 || qpdf_oh_has_key(self.data, dict, key.as_ptr()) == 0 {
                return None;
            }
            let value = qpdf_oh_get_key(self.data, dict, key.as_ptr());
            (qpdf_oh_is_initialized(self.data, value) != 0).then_some(Object(value))
        }
    }

    /// The keys of a dictionary, without their slashes.
    pub fn keys(&self, object: Object) -> Vec<String> {
        // SAFETY: a live handle; each key is copied before the next call.
        unsafe {
            if qpdf_oh_is_dictionary(self.data, object.0) == 0 {
                return Vec::new();
            }
            let mut keys = Vec::new();
            qpdf_oh_begin_dict_key_iter(self.data, object.0);
            while qpdf_oh_dict_more_keys(self.data) != 0 {
                let key = qpdf_oh_dict_next_key(self.data);
                if !key.is_null() {
                    let key = CStr::from_ptr(key).to_string_lossy();
                    keys.push(key.trim_start_matches('/').to_string());
                }
            }
            keys
        }
    }

    pub fn is_stream(&self, object: Object) -> bool {
        // SAFETY: a live handle and object.
        unsafe { qpdf_oh_is_stream(self.data, object.0) != 0 }
    }

    pub fn name(&self, object: Object) -> Option<String> {
        // SAFETY: a live handle and object; the name is copied at once.
        unsafe {
            if qpdf_oh_is_name(self.data, object.0) == 0 {
                return None;
            }
            let name = qpdf_oh_get_name(self.data, object.0);
            (!name.is_null()).then(|| CStr::from_ptr(name).to_string_lossy().trim_start_matches('/').to_string())
        }
    }

    pub fn number(&self, object: Object) -> Option<f64> {
        // SAFETY: a live handle and object.
        unsafe { (qpdf_oh_is_number(self.data, object.0) != 0).then(|| qpdf_oh_get_numeric_value(self.data, object.0)) }
    }

    pub fn integer(&self, object: Object) -> Option<i32> {
        // SAFETY: a live handle and object.
        unsafe {
            (qpdf_oh_is_number(self.data, object.0) != 0).then(|| qpdf_oh_get_int_value_as_int(self.data, object.0))
        }
    }

    pub fn boolean(&self, object: Object) -> Option<bool> {
        // SAFETY: a live handle and object.
        unsafe { (qpdf_oh_is_bool(self.data, object.0) != 0).then(|| qpdf_oh_get_bool_value(self.data, object.0) != 0) }
    }

    pub fn items(&self, object: Object) -> Option<Vec<Object>> {
        // SAFETY: a live handle and object.
        unsafe {
            if qpdf_oh_is_array(self.data, object.0) == 0 {
                return None;
            }
            let n = qpdf_oh_get_array_n_items(self.data, object.0);
            Some((0..n).map(|i| Object(qpdf_oh_get_array_item(self.data, object.0, i))).collect())
        }
    }

    /// Which object this is, the same however it was reached.
    pub fn id(&self, object: Object) -> (i32, i32) {
        // SAFETY: a live handle and object.
        unsafe { (qpdf_oh_get_object_id(self.data, object.0), qpdf_oh_get_generation(self.data, object.0)) }
    }

    /// A stream's data: as stored when `decoded` is false, or with its
    /// general-purpose compression undone when true. `None` if it cannot be
    /// read that way.
    pub fn stream_data(&self, stream: Object, decoded: bool) -> Option<Vec<u8>> {
        let mut filtered: Bool = 0;
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let level = if decoded { DECODE_GENERALIZED } else { DECODE_NONE };
        // SAFETY: a live handle and stream; qpdf hands over a malloc'd
        // buffer, which is copied and freed here.
        unsafe {
            let code = qpdf_oh_get_stream_data(self.data, stream.0, level, &mut filtered, &mut data, &mut len);
            let bytes = (!data.is_null()).then(|| std::slice::from_raw_parts(data, len).to_vec());
            if !data.is_null() {
                libc::free(data.cast());
            }
            if code & QPDF_ERRORS != 0 || (decoded && filtered == 0) {
                // Clear the error, so it is not reported by the next call.
                let _ = self.check(code);
                return None;
            }
            bytes
        }
    }

    /// Replace a picture with a JPEG of `width` × `height`.
    pub fn replace_with_jpeg(&self, image: Object, jpeg: &[u8], width: u32, height: u32) {
        let set = |key: &str, value: Handle| {
            let key = CString::new(format!("/{key}")).expect("keys have no zero bytes");
            // SAFETY: a live handle and object.
            unsafe { qpdf_oh_replace_key(self.data, qpdf_oh_get_dict(self.data, image.0), key.as_ptr(), value) };
        };
        // SAFETY: a live handle and stream; qpdf copies the data.
        unsafe {
            let filter = qpdf_oh_new_name(self.data, c"/DCTDecode".as_ptr());
            let null = qpdf_oh_new_null(self.data);
            qpdf_oh_replace_stream_data(self.data, image.0, jpeg.as_ptr(), jpeg.len(), filter, null);
            set("Width", qpdf_oh_new_integer(self.data, i64::from(width)));
            set("Height", qpdf_oh_new_integer(self.data, i64::from(height)));
            set("BitsPerComponent", qpdf_oh_new_integer(self.data, 8));
            qpdf_oh_remove_key(self.data, qpdf_oh_get_dict(self.data, image.0), c"/DecodeParms".as_ptr());
        }
    }
}

/// A value to put into a new object.
pub enum Value<'a> {
    Name(&'a str),
    Integer(i64),
    Real(f64),
    Object(Object),
    Array(Vec<Value<'a>>),
}

/// Building new objects, for pages written afresh.
impl Qpdf {
    pub fn root(&self) -> Object {
        // SAFETY: a live handle.
        Object(unsafe { qpdf_get_root(self.data) })
    }

    fn value(&self, value: &Value<'_>) -> Handle {
        // SAFETY: a live handle; every string outlives its call.
        unsafe {
            match value {
                Value::Name(name) => {
                    let name = CString::new(format!("/{name}")).expect("names have no zero bytes");
                    qpdf_oh_new_name(self.data, name.as_ptr())
                }
                Value::Integer(n) => qpdf_oh_new_integer(self.data, *n),
                Value::Real(r) => qpdf_oh_new_real_from_double(self.data, *r, 4),
                Value::Object(object) => object.0,
                Value::Array(items) => {
                    let array = qpdf_oh_new_array(self.data);
                    for item in items {
                        qpdf_oh_append_item(self.data, array, self.value(item));
                    }
                    array
                }
            }
        }
    }

    /// Set a key of a dictionary, or of a stream's dictionary.
    pub fn set(&self, object: Object, key: &str, value: Value<'_>) {
        let key = CString::new(format!("/{key}")).expect("keys have no zero bytes");
        let value = self.value(&value);
        // SAFETY: a live handle and object.
        unsafe {
            let dict =
                if qpdf_oh_is_stream(self.data, object.0) != 0 { qpdf_oh_get_dict(self.data, object.0) } else { object.0 };
            qpdf_oh_replace_key(self.data, dict, key.as_ptr(), value);
        }
    }

    pub fn remove(&self, object: Object, key: &str) {
        let key = CString::new(format!("/{key}")).expect("keys have no zero bytes");
        // SAFETY: a live handle and object.
        unsafe {
            let dict =
                if qpdf_oh_is_stream(self.data, object.0) != 0 { qpdf_oh_get_dict(self.data, object.0) } else { object.0 };
            qpdf_oh_remove_key(self.data, dict, key.as_ptr());
        }
    }

    /// A new dictionary, written as an object of its own.
    pub fn new_dictionary(&self, entries: Vec<(&str, Value<'_>)>) -> Object {
        // SAFETY: a live handle.
        let dict = Object(unsafe { qpdf_make_indirect_object(self.data, qpdf_oh_new_dictionary(self.data)) });
        for (key, value) in entries {
            self.set(dict, key, value);
        }
        dict
    }

    /// A new stream of `data`, stored as given: qpdf compresses it on writing
    /// unless `filter` says it is already encoded.
    pub fn new_stream(&self, data: &[u8], filter: Option<&str>, entries: Vec<(&str, Value<'_>)>) -> Object {
        // SAFETY: a live handle; qpdf copies the data.
        let stream = unsafe {
            let stream = qpdf_oh_new_stream(self.data);
            let filter = match filter {
                Some(name) => self.value(&Value::Name(name)),
                None => qpdf_oh_new_null(self.data),
            };
            qpdf_oh_replace_stream_data(self.data, stream, data.as_ptr(), data.len(), filter, qpdf_oh_new_null(self.data));
            Object(stream)
        };
        for (key, value) in entries {
            self.set(stream, key, value);
        }
        stream
    }
}

impl Drop for Qpdf {
    fn drop(&mut self) {
        // SAFETY: the handle is ours, and nothing uses it after this.
        unsafe {
            qpdf_oh_release_all(self.data);
            qpdf_cleanup(&mut self.data);
        }
    }
}
