//! Communicate with smart cards using the PC/SC API.
//!
//! PC/SC (Personal Computer/Smart Card) is a standard API for
//! communicating with smart cards -- enumerating card readers, connecting
//! to smart cards, sending them commands, etc. See [Wikipedia][1] and
//! [PC/SC Workgroup][2] for more information.
//!
//! [1]: https://en.wikipedia.org/wiki/PC/SC
//! [2]: https://www.pcscworkgroup.com/
//!
//! This library is a safe and ergonomic FFI wrapper around the following
//! PC/SC implementations:
//!
//! - On Windows, the built-in `WinSCard.dll` library and "Smart Card"
//!   service. See [MSDN][3] for documentation of the implemented API.
//!
//! - On Apple, the built-in PCSC framework.
//!
//! - On Linux, BSDs and (hopefully) other systems, the PCSC lite library
//!   and pcscd daemon. See [pcsclite][4] for documentation of the
//!   implemented API.
//!
//! [3]: https://msdn.microsoft.com/EN-US/library/aa374731.aspx#smart_card_functions
//! [4]: https://pcsclite.alioth.debian.org/pcsclite.html
//!
//! ## Communicating with a smart card
//!
//! To communicate with a smart card, you send it APDU (Application
//! Protocol Data Unit) commands, and receive APDU responses.
//!
//! The format of these commands is described in the [ISO 7816 Part 4][5]
//! standard. The commands themselves vary based on the application on the
//! card.
//!
//! [5]: http://www.cardwerk.com/smartcards/smartcard_standard_ISO7816-4.aspx
//!
//! ## Note on portability
//!
//! The various implementations are not fully consistent with each other,
//! and some may also miss various features or exhibit various bugs.
//! Hence, you cannot assume that code which works on one platform will
//! behave the same in all other platforms - unfortunately, some
//! adjustments might be needed to reach a common base. See [pcsclite][4]
//! for a list of documented differences, and [Ludovic Rousseau's blog][5]
//! archive for many more details.
//!
//! [6]: https://ludovicrousseau.blogspot.com/
//!
//! Not all PC/SC functionality is covered yet; if you are missing
//! something, please open an issue.
//!
//! ## Note on strings
//!
//! The library uses C strings (`&CStr`) for all strings (e.g. card reader
//! names), to avoid any allocation and conversion overhead.
//!
//! In pcsclite and Apple, all strings are guaranteed to be UTF-8 encoded.
//!
//! In Windows, the API provides two variants of all functions dealing
//! with strings - ASCII and Unicode (in this case, meaning 16-bits wide
//! strings). For ease of implementation, this library wraps the ASCII
//! variants only. (If you require Unicode names in Windows, please open
//! an issue.)
//!
//! Since ASCII is a subset of UTF-8, you can thus safely use UTF-8
//! conversion functions such as `to_str()` to obtain an `&str`/`String`
//! from this library -- but don't do this if you don't need to ☺

#[macro_use]
extern crate bitflags;

use std::os::raw::c_char;
use std::ffi::{CStr, CString};
use std::mem::{transmute, uninitialized, forget};
use std::ptr::{null, null_mut};
use std::marker::PhantomData;
use std::ops::Deref;

mod ffi;
use ffi::{DWORD, LONG};

bitflags! {
    /// A mask of the state a card reader.
    pub flags State: DWORD {
        const STATE_UNAWARE = ffi::SCARD_STATE_UNAWARE,
        const STATE_IGNORE = ffi::SCARD_STATE_IGNORE,
        const STATE_CHANGED = ffi::SCARD_STATE_CHANGED,
        const STATE_UNKNOWN = ffi::SCARD_STATE_UNKNOWN,
        const STATE_UNAVAILABLE = ffi::SCARD_STATE_UNAVAILABLE,
        const STATE_EMPTY = ffi::SCARD_STATE_EMPTY,
        const STATE_PRESENT = ffi::SCARD_STATE_PRESENT,
        const STATE_ATRMATCH = ffi::SCARD_STATE_ATRMATCH,
        const STATE_EXCLUSIVE = ffi::SCARD_STATE_EXCLUSIVE,
        const STATE_INUSE = ffi::SCARD_STATE_INUSE,
        const STATE_MUTE = ffi::SCARD_STATE_MUTE,
        const STATE_UNPOWERED = ffi::SCARD_STATE_UNPOWERED,
    }
}

bitflags! {
    /// A mask of the status of a card in a card reader.
    pub flags Status: DWORD {
        const STATUS_UNKNOWN = ffi::SCARD_UNKNOWN,
        const STATUS_ABSENT = ffi::SCARD_ABSENT,
        const STATUS_PRESENT = ffi::SCARD_PRESENT,
        const STATUS_SWALLOWED = ffi::SCARD_SWALLOWED,
        const STATUS_POWERED = ffi::SCARD_POWERED,
        const STATUS_NEGOTIABLE = ffi::SCARD_NEGOTIABLE,
        const STATUS_SPECIFIC = ffi::SCARD_SPECIFIC,
    }
}

/// How a reader connection is shared.
#[repr(C)]
#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum ShareMode {
    Exclusive = ffi::SCARD_SHARE_EXCLUSIVE as isize,
    Shared = ffi::SCARD_SHARE_SHARED as isize,
    Direct = ffi::SCARD_SHARE_DIRECT as isize,
}

/// A smart card communication protocol.
#[repr(C)]
#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum Protocol {
    T0 = ffi::SCARD_PROTOCOL_T0 as isize,
    T1 = ffi::SCARD_PROTOCOL_T1 as isize,
    RAW = ffi::SCARD_PROTOCOL_RAW as isize,
}

impl Protocol {
    fn from_raw(raw: DWORD) -> Protocol {
        match raw {
            ffi::SCARD_PROTOCOL_T0 => Protocol::T0,
            ffi::SCARD_PROTOCOL_T1 => Protocol::T1,
            ffi::SCARD_PROTOCOL_RAW => Protocol::RAW,
            // This should not be possible, since we only allow to select
            // from Protocol's variants. Hence, we can panic.
            _ => panic!("impossible protocol: {:#x}", raw),
        }
    }
}

bitflags! {
    /// A mask of possible communication protocols.
    pub flags Protocols: DWORD {
        const PROTOCOL_UNDEFINED = ffi::SCARD_PROTOCOL_UNDEFINED,
        const PROTOCOL_T0 = ffi::SCARD_PROTOCOL_T0,
        const PROTOCOL_T1 = ffi::SCARD_PROTOCOL_T1,
        const PROTOCOL_RAW = ffi::SCARD_PROTOCOL_RAW,
        const PROTOCOL_ANY = ffi::SCARD_PROTOCOL_ANY,
    }
}

/// Disposition method when disconnecting from a card reader.
#[repr(C)]
#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum Disposition {
    LeaveCard = ffi::SCARD_LEAVE_CARD as isize,
    ResetCard = ffi::SCARD_RESET_CARD as isize,
    UnpowerCard = ffi::SCARD_UNPOWER_CARD as isize,
    EjectCard = ffi::SCARD_EJECT_CARD as isize,
}

/// Possible library errors.
///
/// See [pcsclite][1], [MSDN][2].
///
/// [1]: https://pcsclite.alioth.debian.org/api/group__ErrorCodes.html
/// [2]: https://msdn.microsoft.com/en-us/library/windows/desktop/aa374738(v=vs.85).aspx#smart_card_return_values
#[repr(u32)]
#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum Error {
    // <contiguous block 1>
    InternalError = ffi::SCARD_F_INTERNAL_ERROR as u32,
    Cancelled = ffi::SCARD_E_CANCELLED as u32,
    InvalidHandle = ffi::SCARD_E_INVALID_HANDLE as u32,
    InvalidParameter = ffi::SCARD_E_INVALID_PARAMETER as u32,
    InvalidTarget = ffi::SCARD_E_INVALID_TARGET as u32,
    NoMemory = ffi::SCARD_E_NO_MEMORY as u32,
    WaitedTooLong = ffi::SCARD_F_WAITED_TOO_LONG as u32,
    InsufficientBuffer = ffi::SCARD_E_INSUFFICIENT_BUFFER as u32,
    UnknownReader = ffi::SCARD_E_UNKNOWN_READER as u32,
    Timeout = ffi::SCARD_E_TIMEOUT as u32,
    SharingViolation = ffi::SCARD_E_SHARING_VIOLATION as u32,
    NoSmartcard = ffi::SCARD_E_NO_SMARTCARD as u32,
    UnknownCard = ffi::SCARD_E_UNKNOWN_CARD as u32,
    CantDispose = ffi::SCARD_E_CANT_DISPOSE as u32,
    ProtoMismatch = ffi::SCARD_E_PROTO_MISMATCH as u32,
    NotReady = ffi::SCARD_E_NOT_READY as u32,
    InvalidValue = ffi::SCARD_E_INVALID_VALUE as u32,
    SystemCancelled = ffi::SCARD_E_SYSTEM_CANCELLED as u32,
    CommError = ffi::SCARD_F_COMM_ERROR as u32,
    UnknownError = ffi::SCARD_F_UNKNOWN_ERROR as u32,
    InvalidAtr = ffi::SCARD_E_INVALID_ATR as u32,
    NotTransacted = ffi::SCARD_E_NOT_TRANSACTED as u32,
    ReaderUnavailable = ffi::SCARD_E_READER_UNAVAILABLE as u32,
    Shutdown = ffi::SCARD_P_SHUTDOWN as u32,
    PciTooSmall = ffi::SCARD_E_PCI_TOO_SMALL as u32,
    ReaderUnsupported = ffi::SCARD_E_READER_UNSUPPORTED as u32,
    DuplicateReader = ffi::SCARD_E_DUPLICATE_READER as u32,
    CardUnsupported = ffi::SCARD_E_CARD_UNSUPPORTED as u32,
    NoService = ffi::SCARD_E_NO_SERVICE as u32,
    ServiceStopped = ffi::SCARD_E_SERVICE_STOPPED as u32,
    Unexpected = ffi::SCARD_E_UNEXPECTED as u32,
    IccInstallation = ffi::SCARD_E_ICC_INSTALLATION as u32,
    IccCreateorder = ffi::SCARD_E_ICC_CREATEORDER as u32,
    UnsupportedFeature = ffi::SCARD_E_UNSUPPORTED_FEATURE as u32,
    DirNotFound = ffi::SCARD_E_DIR_NOT_FOUND as u32,
    FileNotFound = ffi::SCARD_E_FILE_NOT_FOUND as u32,
    NoDir = ffi::SCARD_E_NO_DIR as u32,
    NoFile = ffi::SCARD_E_NO_FILE as u32,
    NoAccess = ffi::SCARD_E_NO_ACCESS as u32,
    WriteTooMany = ffi::SCARD_E_WRITE_TOO_MANY as u32,
    BadSeek = ffi::SCARD_E_BAD_SEEK as u32,
    InvalidChv = ffi::SCARD_E_INVALID_CHV as u32,
    UnknownResMng = ffi::SCARD_E_UNKNOWN_RES_MNG as u32,
    NoSuchCertificate = ffi::SCARD_E_NO_SUCH_CERTIFICATE as u32,
    CertificateUnavailable = ffi::SCARD_E_CERTIFICATE_UNAVAILABLE as u32,
    NoReadersAvailable = ffi::SCARD_E_NO_READERS_AVAILABLE as u32,
    CommDataLost = ffi::SCARD_E_COMM_DATA_LOST as u32,
    NoKeyContainer = ffi::SCARD_E_NO_KEY_CONTAINER as u32,
    ServerTooBusy = ffi::SCARD_E_SERVER_TOO_BUSY as u32,
    // </contiguous block 1>

    // <contiguous block 2>
    UnsupportedCard = ffi::SCARD_W_UNSUPPORTED_CARD as u32,
    UnresponsiveCard = ffi::SCARD_W_UNRESPONSIVE_CARD as u32,
    UnpoweredCard = ffi::SCARD_W_UNPOWERED_CARD as u32,
    ResetCard = ffi::SCARD_W_RESET_CARD as u32,
    RemovedCard = ffi::SCARD_W_REMOVED_CARD as u32,

    SecurityViolation = ffi::SCARD_W_SECURITY_VIOLATION as u32,
    WrongChv = ffi::SCARD_W_WRONG_CHV as u32,
    ChvBlocked = ffi::SCARD_W_CHV_BLOCKED as u32,
    Eof = ffi::SCARD_W_EOF as u32,
    CancelledByUser = ffi::SCARD_W_CANCELLED_BY_USER as u32,
    CardNotAuthenticated = ffi::SCARD_W_CARD_NOT_AUTHENTICATED as u32,

    CacheItemNotFound = ffi::SCARD_W_CACHE_ITEM_NOT_FOUND as u32,
    CacheItemStale = ffi::SCARD_W_CACHE_ITEM_STALE as u32,
    CacheItemTooBig = ffi::SCARD_W_CACHE_ITEM_TOO_BIG as u32,
    // </contiguous block 2>
}

impl Error {
    fn from_raw(raw: LONG) -> Error {
        unsafe {
            // The ranges here are the "blocks" above.
            if ffi::SCARD_F_INTERNAL_ERROR <= raw && raw <= ffi::SCARD_E_SERVER_TOO_BUSY ||
                ffi::SCARD_W_UNSUPPORTED_CARD <= raw && raw <= ffi::SCARD_W_CACHE_ITEM_TOO_BIG {
                transmute(raw as u32)
            } else {
                debug_assert!(false, format!("unknown PCSC error code: {:#x}", raw));
                // We mask unknown error codes here; this is not very nice,
                // but seems better than panicking.
                Error::UnknownError
            }
        }
    }
}

macro_rules! try_pcsc {
    ($e:expr) => (match $e {
        ffi::SCARD_S_SUCCESS => (),
        err => return Err(Error::from_raw(err)),
    });
}

/// Scope of a context.
#[repr(C)]
#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum Scope {
    User = ffi::SCARD_SCOPE_USER as isize,
    Terminal = ffi::SCARD_SCOPE_TERMINAL as isize,
    System = ffi::SCARD_SCOPE_SYSTEM as isize,
    Global = ffi::SCARD_SCOPE_GLOBAL as isize,
}

/// A class of Attributes.
#[repr(C)]
#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum AttributeClass {
    VendorInfo = ffi::SCARD_CLASS_VENDOR_INFO as isize,
    Communications = ffi::SCARD_CLASS_COMMUNICATIONS as isize,
    Protocol = ffi::SCARD_CLASS_PROTOCOL as isize,
    PowerMgmt = ffi::SCARD_CLASS_POWER_MGMT as isize,
    Security = ffi::SCARD_CLASS_SECURITY as isize,
    Mechanical = ffi::SCARD_CLASS_MECHANICAL as isize,
    VendorDefined = ffi::SCARD_CLASS_VENDOR_DEFINED as isize,
    IfdProtocol = ffi::SCARD_CLASS_IFD_PROTOCOL as isize,
    IccState = ffi::SCARD_CLASS_ICC_STATE as isize,
    System = ffi::SCARD_CLASS_SYSTEM as isize,
}

/// Card reader attribute types.
#[repr(C)]
#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum Attribute {
    VendorName = ffi::SCARD_ATTR_VENDOR_NAME as isize,
    VendorIfdType = ffi::SCARD_ATTR_VENDOR_IFD_TYPE as isize,
    VendorIfdVersion = ffi::SCARD_ATTR_VENDOR_IFD_VERSION as isize,
    VendorIfdSerialNo = ffi::SCARD_ATTR_VENDOR_IFD_SERIAL_NO as isize,
    ChannelId = ffi::SCARD_ATTR_CHANNEL_ID as isize,
    AsyncProtocolTypes = ffi::SCARD_ATTR_ASYNC_PROTOCOL_TYPES as isize,
    DefaultClk = ffi::SCARD_ATTR_DEFAULT_CLK as isize,
    MaxClk = ffi::SCARD_ATTR_MAX_CLK as isize,
    DefaultDataRate = ffi::SCARD_ATTR_DEFAULT_DATA_RATE as isize,
    MaxDataRate = ffi::SCARD_ATTR_MAX_DATA_RATE as isize,
    MaxIfsd = ffi::SCARD_ATTR_MAX_IFSD as isize,
    SyncProtocolTypes = ffi::SCARD_ATTR_SYNC_PROTOCOL_TYPES as isize,
    PowerMgmtSupport = ffi::SCARD_ATTR_POWER_MGMT_SUPPORT as isize,
    UserToCardAuthDevice = ffi::SCARD_ATTR_USER_TO_CARD_AUTH_DEVICE as isize,
    UserAuthInputDevice = ffi::SCARD_ATTR_USER_AUTH_INPUT_DEVICE as isize,
    Characteristics = ffi::SCARD_ATTR_CHARACTERISTICS as isize,

    CurrentProtocolType = ffi::SCARD_ATTR_CURRENT_PROTOCOL_TYPE as isize,
    CurrentClk = ffi::SCARD_ATTR_CURRENT_CLK as isize,
    CurrentF = ffi::SCARD_ATTR_CURRENT_F as isize,
    CurrentD = ffi::SCARD_ATTR_CURRENT_D as isize,
    CurrentN = ffi::SCARD_ATTR_CURRENT_N as isize,
    CurrentW = ffi::SCARD_ATTR_CURRENT_W as isize,
    CurrentIfsc = ffi::SCARD_ATTR_CURRENT_IFSC as isize,
    CurrentIfsd = ffi::SCARD_ATTR_CURRENT_IFSD as isize,
    CurrentBwt = ffi::SCARD_ATTR_CURRENT_BWT as isize,
    CurrentCwt = ffi::SCARD_ATTR_CURRENT_CWT as isize,
    CurrentEbcEncoding = ffi::SCARD_ATTR_CURRENT_EBC_ENCODING as isize,
    ExtendedBwt = ffi::SCARD_ATTR_EXTENDED_BWT as isize,

    IccPresence = ffi::SCARD_ATTR_ICC_PRESENCE as isize,
    IccInterfaceStatus = ffi::SCARD_ATTR_ICC_INTERFACE_STATUS as isize,
    CurrentIoState = ffi::SCARD_ATTR_CURRENT_IO_STATE as isize,
    AtrString = ffi::SCARD_ATTR_ATR_STRING as isize,
    IccTypePerAtr = ffi::SCARD_ATTR_ICC_TYPE_PER_ATR as isize,

    EscReset = ffi::SCARD_ATTR_ESC_RESET as isize,
    EscCancel = ffi::SCARD_ATTR_ESC_CANCEL as isize,
    EscAuthrequest = ffi::SCARD_ATTR_ESC_AUTHREQUEST as isize,
    Maxinput = ffi::SCARD_ATTR_MAXINPUT as isize,

    DeviceUnit = ffi::SCARD_ATTR_DEVICE_UNIT as isize,
    DeviceInUse = ffi::SCARD_ATTR_DEVICE_IN_USE as isize,
    DeviceFriendlyName = ffi::SCARD_ATTR_DEVICE_FRIENDLY_NAME as isize,
    DeviceSystemName = ffi::SCARD_ATTR_DEVICE_SYSTEM_NAME as isize,
    SupressT1IfsRequest = ffi::SCARD_ATTR_SUPRESS_T1_IFS_REQUEST as isize,
}

/// Maximum amount of bytes in an ATR.
pub const MAX_ATR_SIZE: usize = 33;
/// Maximum amount of bytes in a short APDU command or response.
pub const MAX_BUFFER_SIZE: usize = 264;
/// Maximum amount of bytes in an extended APDU command or response.
pub const MAX_BUFFER_SIZE_EXTENDED: usize = 4 + 3 + (1 << 16) + 3 + 2;

/// A special value for detecting card reader insertions and removals.
///
/// # Note
///
/// This function is a wrapper around a constant, and is intended to be
/// used as such.
#[allow(non_snake_case)]
// We can't have a const &CStr yet, so we simulate it with a function.
pub fn PNP_NOTIFICATION() -> &'static CStr {
    unsafe { CStr::from_bytes_with_nul_unchecked(b"\\\\?PnP?\\Notification\0") }
}

/// A structure for tracking the current state of card readers and cards.
///
/// This structure wraps `SCARD_READERSTATE` ([pcsclite][1], [MSDN][2]).
///
/// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga33247d5d1257d59e55647c3bb717db24
/// [2]: https://msdn.microsoft.com/en-us/library/aa379808.aspx
#[repr(C)]
pub struct ReaderState {
    // Note: must be directly transmutable to SCARD_READERSTATE.
    inner: ffi::SCARD_READERSTATE,
}

// For some reason, linking in windows fails if we put these directly
// in statics. This is why we have this function instead of the
// SCARD_PCI_* defines from the C API.
fn get_protocol_pci(protocol: Protocol) -> &'static ffi::SCARD_IO_REQUEST {
    unsafe {
        match protocol {
            Protocol::T0 => &ffi::g_rgSCardT0Pci,
            Protocol::T1 => &ffi::g_rgSCardT1Pci,
            Protocol::RAW => &ffi::g_rgSCardRawPci,
        }
    }
}

/// Library context to the PCSC service.
///
/// This structure wraps `SCARDCONTEXT`.
pub struct Context {
    // A context and all derived objects must only be used in
    // the thread which created it.
    // We should use negative impls (!Sync, !Send) if they stabilize.
    _not_sync_send: PhantomData<*const ()>,
    handle: ffi::SCARDCONTEXT,
}

/// A structures that can be moved to another thread to allow it to cancel
/// a blocking operation in the Context.
///
/// # Note
///
/// Cancelers are intentionally not tied to the lifetime of the Context
/// in which they were created, since that will hinder their use.
///
/// This means that it is possible to use a Canceler after the Context
/// is already dead. In this case, `cancel()` will return
/// `Error::InvalidHandle`.
pub struct Canceler {
    handle: ffi::SCARDCONTEXT,
}

/// A connection to a smart card.
///
/// This structure wraps `SCARDHANDLE`.
pub struct Card<'ctx> {
    _context: PhantomData<&'ctx Context>,
    handle: ffi::SCARDHANDLE,
    active_protocol: Protocol,
}

/// An exclusive transaction with a card.
pub struct Transaction<'card> {
    card: &'card Card<'card>,
}

/// An iterator over card reader names.
///
/// The iterator does not perform any copying or allocations; this is left
/// to the caller's discretion. It is therefore tied to the underlying
/// buffer.
#[derive(Clone)]
pub struct ReaderNames<'buf> {
    buf: &'buf [u8],
    pos: usize,
}

impl<'buf> Iterator for ReaderNames<'buf> {
    type Item = &'buf CStr;

    fn next(&mut self) -> Option<&'buf CStr> {
        match self.buf[self.pos..].iter().position(|&c| c == 0) {
            None | Some(0) => None,
            Some(len) => unsafe {
                let old_pos = self.pos;
                self.pos += len + 1;
                Some(CStr::from_bytes_with_nul_unchecked(&self.buf[old_pos..self.pos]))
            }
        }
    }
}

// TODO: Maybe some methods should take `&mut self` instead of `&self`?

impl Context {
    /// Establish a new context.
    ///
    /// This function wraps `SCardEstablishContext` ([pcsclite][1],
    /// [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#gaa1b8970169fd4883a6dc4a8f43f19b67
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379479.aspx
    pub fn establish(
        scope: Scope,
    ) -> Result<Context, Error> {
        unsafe {
            let mut ctx: ffi::SCARDCONTEXT = uninitialized();

            try_pcsc!(ffi::SCardEstablishContext(
                scope as DWORD,
                null(),
                null(),
                &mut ctx,
            ));
            Ok(Context{
                _not_sync_send: PhantomData,
                handle: ctx,
            })
        }
    }

    /// Release the context.
    ///
    /// In case of error, ownership of the context is returned to the
    /// caller.
    ///
    /// This function wraps `SCardReleaseContext` ([pcsclite][1],
    /// [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga6aabcba7744c5c9419fdd6404f73a934
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379798.aspx
    ///
    /// ## Note
    ///
    /// `Context` implements `Drop` which automatically releases the
    /// context; you only need to call this function if you want to handle
    /// errors.
    pub fn release(
        self
    ) -> Result<(), (Context, Error)> {
        unsafe {
            let err = ffi::SCardReleaseContext(
                self.handle,
            );
            if err != ffi::SCARD_S_SUCCESS {
                return Err((self, Error::from_raw(err)));
            }

            // Skip the drop, we did it "manually".
            forget(self);

            Ok(())
        }
    }

    /// Check whether the Context is still valid.
    ///
    /// This function wraps `SCardIsValidContext` ([pcsclite][1],
    /// [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga722eb66bcc44d391f700ff9065cc080b
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379788.aspx
    pub fn is_valid(
        &self
    ) -> Result<(), Error> {
        unsafe {
            try_pcsc!(ffi::SCardIsValidContext(
                self.handle,
            ));
            Ok(())
        }
    }

    /// Get a Canceler for this `Context`.
    ///
    /// The Canceler can be passed to another thread to allow that thread
    /// to cancel an ongoing blocking operation on the `Context`.
    ///
    /// See the `cancel.rs` example program.
    pub fn get_canceler(
        &self
    ) -> Canceler {
        Canceler {
            handle: self.handle,
        }
    }

    /// List all connected card readers.
    ///
    /// `buffer` is a buffer that should be large enough to hold all of
    /// the connected reader names.
    ///
    /// Returns an iterator over the reader names. The iterator yields
    /// values directly from `buffer`.
    ///
    /// If the buffer is not large enough to hold all of the names,
    /// `Error::InsufficientBuffer` is returned.
    ///
    /// This function wraps `SCardListReaders` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga93b07815789b3cf2629d439ecf20f0d9
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379793.aspx
    // TODO: Add way to safely get the needed buffer size (returned in
    // buflen).
    pub fn list_readers<'buf>(
        &self,
        buffer: &'buf mut [u8],
    ) -> Result<ReaderNames<'buf>, Error> {
        unsafe {
            let mut buflen = buffer.len() as DWORD;

            let err = ffi::SCardListReaders(
                self.handle,
                null(),
                buffer.as_mut_ptr() as *mut c_char,
                &mut buflen,
            );
            if err == Error::NoReadersAvailable as LONG {
                return Ok(ReaderNames {
                    buf: b"\0",
                    pos: 0,
                });
            }
            if err != ffi::SCARD_S_SUCCESS {
                return Err(Error::from_raw(err));
            }

            Ok(ReaderNames{
                buf: &buffer[..buflen as usize],
                pos: 0,
            })
        }
    }

    /// Connect to a card which is present in a reader.
    ///
    /// See the `connect.rs` example program.
    ///
    /// This function wraps `SCardConnect` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga4e515829752e0a8dbc4d630696a8d6a5
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379473.aspx
    pub fn connect(
        &self,
        reader: &CStr,
        share_mode: ShareMode,
        preferred_protocols: Protocols,
    ) -> Result<Card, Error> {
        unsafe {
            let mut handle: ffi::SCARDHANDLE = uninitialized();
            let mut raw_active_protocol: DWORD = uninitialized();

            try_pcsc!(ffi::SCardConnect(
                self.handle,
                reader.as_ptr(),
                share_mode as DWORD,
                preferred_protocols.bits(),
                &mut handle,
                &mut raw_active_protocol,
            ));

            let active_protocol = Protocol::from_raw(raw_active_protocol);

            Ok(Card{
                _context: PhantomData,
                handle: handle,
                active_protocol: active_protocol,
            })
        }
    }

    /// Wait for card and card reader state changes.
    ///
    /// The function blocks until the state of one of the readers changes
    /// from corresponding passed-in `ReaderState`. The `ReaderState`s are
    /// updated to report the new state.
    ///
    /// A special reader name, `\\?PnP?\Notification`, can be used to
    /// detect card reader insertions and removals, as opposed to state
    /// changes of a specific reader. Use `PNP_NOTIFICATION()` to easily
    /// obtain a static reference to this name.
    ///
    /// See the `monitor.rs` example program.
    ///
    /// This function wraps `SCardGetStatusChange` ([pcsclite][1],
    /// [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga33247d5d1257d59e55647c3bb717db24
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379773.aspx
    pub fn get_status_change<D>(
        &self,
        timeout: D,
        readers: &mut [ReaderState],
    ) -> Result<(), Error>
        where D: Into<Option<std::time::Duration>> {
        let timeout_ms = match timeout.into() {
            Some(duration) => {
                let timeout_ms_u64 = duration.as_secs()
                    .saturating_mul(1000)
                    .saturating_add(duration.subsec_nanos() as u64 / 1_000_000);
                std::cmp::min(ffi::INFINITE, timeout_ms_u64 as DWORD)
            },
            None => ffi::INFINITE
        };

        unsafe {
            try_pcsc!(ffi::SCardGetStatusChange(
                self.handle,
                timeout_ms,
                transmute(readers.as_mut_ptr()),
                readers.len() as DWORD,
            ));

            Ok(())
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            // Error is ignored here; to do proper error handling,
            // release() should be called manually.
            let _err = ffi::SCardReleaseContext(
                self.handle,
            );
        }
    }
}

impl ReaderState {
    /// Create a ReaderState for a card reader with a given presumed
    /// state.
    ///
    /// ## Note
    ///
    /// This function allocates a copy of `name`, so that the returned
    /// `ReaderState` is not tied to `name`'s lifetime'; it would have
    /// been difficult to use `Context::get_status_changes` otherwise.
    // TODO: Support ATR fields.
    pub fn new(
        name: &CStr,
        current_state: State,
    ) -> ReaderState {
        ReaderState {
            inner: ffi::SCARD_READERSTATE {
                szReader: name.to_owned().into_raw(),
                // This seems useless to expose.
                pvUserData: null_mut(),
                dwCurrentState: current_state.bits(),
                dwEventState: STATE_UNAWARE.bits(),
                cbAtr: 0,
                rgbAtr: [0; ffi::ATR_BUFFER_SIZE],
            },
        }
    }

    /// The name of the card reader.
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.inner.szReader) }
    }

    /// The last reported state.
    pub fn event_state(&self) -> State {
        State::from_bits_truncate(self.inner.dwEventState)
    }

    /// The card event count.
    ///
    /// The count is incremented for each card insertion or removal in the
    /// reader. This can be used to detect a card removal/insertion
    /// between two calls to `Context::get_status_change()`.
    pub fn event_count(&self) -> u32 {
        ((self.inner.dwEventState & 0xFFFF0000) >> 16) as u32
    }

    /// Sync the currently-known state to the last reported state.
    pub fn sync_current_state(&mut self) {
        // In windows it is important that the event count is included;
        // otherwise PNP_NOTIFICATION is always reported as changed:
        // https://stackoverflow.com/a/16467368
        self.inner.dwCurrentState = self.inner.dwEventState;
    }
}

impl Drop for ReaderState {
    fn drop(&mut self) {
        // Reclaim the name and drop it immediately.
        unsafe { CString::from_raw(self.inner.szReader as *mut c_char) };
    }
}

impl<'ctx> Card<'ctx> {
    /// Start a new exclusive transaction with the card.
    ///
    /// Any further operations for the duration of the transaction should
    /// be performed through the returned `Transaction`.
    ///
    /// This function wraps `SCardBeginTransaction` ([pcsclite][1],
    /// [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#gaddb835dce01a0da1d6ca02d33ee7d861
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379469.aspx
    pub fn transaction(
        &mut self,
    ) -> Result<Transaction, Error> {
        unsafe {
            try_pcsc!(ffi::SCardBeginTransaction(
                self.handle,
            ));

            Ok(Transaction{
                card: self,
            })
        }
    }

    /// Reconnect to the card.
    ///
    /// This function wraps `SCardReconnect` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#gad5d4393ca8c470112ad9468c44ed8940
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379797.aspx
    pub fn reconnect(
        &mut self,
        share_mode: ShareMode,
        preferred_protocols: Protocols,
        initialization: Disposition,
    ) -> Result<(), Error> {
        unsafe {
            let mut raw_active_protocol: DWORD = uninitialized();

            try_pcsc!(ffi::SCardReconnect(
                self.handle,
                share_mode as DWORD,
                preferred_protocols.bits(),
                initialization as DWORD,
                &mut raw_active_protocol,
            ));

            self.active_protocol = Protocol::from_raw(raw_active_protocol);

            Ok(())
        }
    }

    /// Disconnect from the card.
    ///
    /// In case of error, ownership of the card is returned to the caller.
    ///
    /// This function wraps `SCardDisconnect` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga4be198045c73ec0deb79e66c0ca1738a
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379475.aspx
    ///
    /// ## Note
    ///
    /// `Card` implements `Drop` which automatically disconnects the card
    /// using `Disposition::ResetCard`; you only need to call this
    /// function if you want to handle errors or use a different
    /// disposition method.
    pub fn disconnect(
        self,
        disposition: Disposition,
    ) -> Result<(), (Card<'ctx>, Error)> {
        unsafe {
            let err = ffi::SCardDisconnect(
                self.handle,
                disposition as DWORD,
            );
            if err != ffi::SCARD_S_SUCCESS {
                return Err((self, Error::from_raw(err)));
            }

            // Skip the drop, we did it "manually".
            forget(self);

            Ok(())
        }
    }

    /// Get current info on the card.
    ///
    /// This function wraps `SCardStatus` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#gae49c3c894ad7ac12a5b896bde70d0382
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379803.aspx
    // TODO: Missing return values: reader names and ATR.
    pub fn status(
        &self,
    ) -> Result<(Status, Protocol), Error> {
        unsafe {
            let mut raw_status: DWORD = uninitialized();
            let mut raw_protocol: DWORD = uninitialized();

            try_pcsc!(ffi::SCardStatus(
                self.handle,
                null_mut(),
                null_mut(),
                &mut raw_status,
                &mut raw_protocol,
                null_mut(),
                null_mut(),
            ));

            let status = Status::from_bits_truncate(raw_status);
            let protocol = Protocol::from_raw(raw_protocol);

            Ok((status, protocol))
        }
    }

    /// Get an attribute of the card or card reader.
    ///
    /// `buffer` is a buffer that should be large enough for the attribute
    /// data.
    ///
    /// Returns a slice into `buffer` containing the attribute data.
    ///
    /// If the buffer is not large enough, `Error::InsufficientBuffer` is
    /// returned.
    ///
    /// This function wraps `SCardGetAttrib` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#gaacfec51917255b7a25b94c5104961602
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379559.aspx
    // TODO: Add way to safely get the needed buffer size (returned in
    // attribute_len).
    pub fn get_attribute<'buf>(
        &self,
        attribute: Attribute,
        buffer: &'buf mut [u8],
    ) -> Result<&'buf [u8], Error> {
        unsafe {
            let mut attribute_len = buffer.len() as DWORD;

            try_pcsc!(ffi::SCardGetAttrib(
                self.handle,
                attribute as DWORD,
                buffer.as_mut_ptr(),
                &mut attribute_len,
            ));

            Ok(&buffer[0..attribute_len as usize])
        }
    }

    /// Set an attribute of the card or card reader.
    ///
    /// This function wraps `SCardSetAttrib` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga060f0038a4ddfd5dd2b8fadf3c3a2e4f
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379801.aspx
    pub fn set_attribute(
        &self,
        attribute: Attribute,
        attribute_data: &[u8],
    ) -> Result<(), Error> {
        unsafe {
            try_pcsc!(ffi::SCardSetAttrib(
                self.handle,
                attribute as DWORD,
                attribute_data.as_ptr(),
                attribute_data.len() as DWORD,
            ));

            Ok(())
        }
    }

    /// Transmit an APDU command to the card.
    ///
    /// `receive_buffer` is a buffer that should be large enough to hold
    /// the APDU response.
    ///
    /// Returns a slice into `receive_buffer` containing the APDU
    /// response.
    ///
    /// If `receive_buffer` is not large enough to hold the APDU response,
    /// `Error::InsufficientBuffer` is returned.
    ///
    /// This function wraps `SCardTransmit` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#ga9a2d77242a271310269065e64633ab99
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379804.aspx
    pub fn transmit<'buf>(
        &self,
        send_buffer: &[u8],
        receive_buffer: &'buf mut [u8],
    ) -> Result<&'buf [u8], Error> {
        let send_pci = get_protocol_pci(self.active_protocol);
        let recv_pci = null_mut();
        let mut receive_len = receive_buffer.len() as DWORD;

        unsafe {
            try_pcsc!(ffi::SCardTransmit(
                self.handle,
                send_pci,
                send_buffer.as_ptr(),
                send_buffer.len() as DWORD,
                recv_pci,
                receive_buffer.as_mut_ptr(),
                &mut receive_len,
            ));

            Ok(&receive_buffer[0..receive_len as usize])
        }
    }
}

impl<'ctx> Drop for Card<'ctx> {
    fn drop(&mut self) {
        unsafe {
            // Error is ignored here; to do proper error handling,
            // disconnect() should be called manually.
            //
            // Disposition is hard-coded to ResetCard here; to use
            // another method, disconnect() should be called manually.
            let _err = ffi::SCardDisconnect(
                self.handle,
                Disposition::ResetCard as DWORD,
            );
        }
    }
}

impl<'card> Transaction<'card> {
    /// End the transaction.
    ///
    /// In case of error, ownership of the transaction is returned to the
    /// caller.
    ///
    /// This function wraps `SCardEndTransaction` ([pcsclite][1],
    /// [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#gae8742473b404363e5c587f570d7e2f3b
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379477.aspx
    ///
    /// ## Note
    ///
    /// `Transaction` implements `Drop` which automatically ends the
    /// transaction using `Disposition::LeaveCard`; you only need to call
    /// this function if you want to handle errors or use a different
    /// disposition method.
    pub fn end(
        self,
        disposition: Disposition,
    ) -> Result<(), (Transaction<'card>, Error)> {
        unsafe {
            let err = ffi::SCardEndTransaction(
                self.card.handle,
                disposition as DWORD,
            );
            if err != 0 {
                return Err((self, Error::from_raw(err)));
            }

            // Skip the drop, we did it "manually".
            forget(self);

            Ok(())
        }
    }
}

impl<'card> Drop for Transaction<'card> {
    fn drop(&mut self) {
        unsafe {
            // Error is ignored here; to do proper error handling,
            // end() should be called manually.
            //
            // Disposition is hard-coded to LeaveCard here; to use
            // another method, end() should be called manually.
            let _err = ffi::SCardEndTransaction(
                self.card.handle,
                Disposition::LeaveCard as DWORD,
            );
        }
    }
}

impl<'card> Deref for Transaction<'card> {
    type Target = Card<'card>;

    fn deref(&self) -> &Card<'card> {
        self.card
    }
}

impl Canceler {
    /// Cancel any ongoing blocking operation in the Context.
    ///
    /// This function wraps `SCardCancel` ([pcsclite][1], [MSDN][2]).
    ///
    /// [1]: https://pcsclite.alioth.debian.org/api/group__API.html#gaacbbc0c6d6c0cbbeb4f4debf6fbeeee6
    /// [2]: https://msdn.microsoft.com/en-us/library/aa379470.aspx
    pub fn cancel(
        &self,
    ) -> Result<(), Error> {
        unsafe {
            try_pcsc!(ffi::SCardCancel(
                self.handle,
            ));

            Ok(())
        }
    }
}

unsafe impl Send for Canceler {}
unsafe impl Sync for Canceler {}
