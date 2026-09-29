// Rust half of the minimal libc shim for embedding vendored Lua (via mlua)
// on `wasm32-unknown-unknown`.
//
// Everything here is gated to `wasm32-unknown-unknown`; on any other target
// this module compiles to nothing.
//
// The C half (see `wasm-shim-c/csrc/`) provides `setjmp`/`longjmp` on top of
// the WebAssembly exception-handling proposal, the stdio/printf family and
// `abort`/`exit`; it is compiled by mlua-sys's build script (main_inner.rs)
// and linked as `liblua_shim.a`.

use core::ffi::{c_char, c_double, c_int, c_long, c_ulong, c_void};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod impls {
    use super::*;

    // ------------------------------------------------------------------
    // errno
    // ------------------------------------------------------------------

    static mut ERRNO: c_int = 0;

    #[no_mangle]
    pub unsafe extern "C" fn __errno_location() -> *mut c_int {
        core::ptr::addr_of_mut!(ERRNO)
    }

    #[no_mangle]
    pub extern "C" fn strerror(_errnum: c_int) -> *const c_char {
        b"Unknown error\0".as_ptr().cast()
    }

    // ------------------------------------------------------------------
    // malloc family — routed through Rust's global allocator (dlmalloc on
    // wasm32-unknown-unknown). Lua's default allocator (lauxlib's l_alloc)
    // calls realloc/malloc/free, and the vendored Lua C objects reference
    // these C names directly.
    // ------------------------------------------------------------------

    const ALIGN: usize = 8;

    #[no_mangle]
    pub unsafe extern "C" fn malloc(size: usize) -> *mut c_void {
        if size == 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: alignment 8 is a power of two and <= 2^31; caller-supplied size.
        unsafe {
            let layout = core::alloc::Layout::from_size_align_unchecked(size, ALIGN);
            std::alloc::alloc(layout).cast()
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn calloc(nmemb: usize, size: usize) -> *mut c_void {
        let total = nmemb.saturating_mul(size);
        let p = unsafe { malloc(total) };
        if !p.is_null() {
            // SAFETY: malloc returned `total` zeroed-capable bytes; zero them.
            unsafe { core::ptr::write_bytes(p, 0, total) };
        }
        p
    }

    #[no_mangle]
    pub unsafe extern "C" fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
        if ptr.is_null() {
            return unsafe { malloc(size) };
        }
        if size == 0 {
            unsafe { free(ptr) };
            return core::ptr::null_mut();
        }
        // SAFETY: all shim allocations use ALIGN (see malloc); dlmalloc's
        // realloc ignores the reported layout size.
        unsafe {
            let layout = core::alloc::Layout::from_size_align_unchecked(size, ALIGN);
            std::alloc::realloc(ptr.cast::<u8>(), layout, size).cast()
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn free(ptr: *mut c_void) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: dealloc only requires a layout whose alignment matches the
        // original allocation (dlmalloc ignores size); we always allocate
        // with ALIGN.
        unsafe {
            let layout = core::alloc::Layout::from_size_align_unchecked(1, ALIGN);
            std::alloc::dealloc(ptr.cast::<u8>(), layout);
        }
    }

    // ------------------------------------------------------------------
    // qsort (heapsort: O(n log n), no recursion, C comparator)
    // ------------------------------------------------------------------

    type Compar = unsafe extern "C" fn(*const c_void, *const c_void) -> c_int;

    #[no_mangle]
    pub unsafe extern "C" fn qsort(
        base: *mut c_void,
        nmemb: usize,
        size: usize,
        compar: Compar,
    ) {
        if nmemb < 2 || size == 0 || base.is_null() {
            return;
        }
        let arr = base as *mut u8;
        let mut tmp = vec![0u8; size];

        // Heapify + sift-down. `tmp` is passed in to avoid capturing it
        // (which would conflict with the direct uses below).
        let sift = |start: usize, end: usize, tmp: &mut [u8]| {
            let mut root = start;
            loop {
                let mut child = root * 2 + 1;
                if child >= end {
                    break;
                }
                if child + 1 < end {
                    let c1 = arr.add(child * size);
                    let c2 = arr.add((child + 1) * size);
                    if compar(c1 as *const _, c2 as *const _) < 0 {
                        child += 1;
                    }
                }
                let rc = arr.add(root * size);
                let cc = arr.add(child * size);
                if compar(rc as *const _, cc as *const _) >= 0 {
                    break;
                }
                core::ptr::copy_nonoverlapping(cc, tmp.as_mut_ptr(), size);
                core::ptr::copy_nonoverlapping(rc, cc, size);
                core::ptr::copy_nonoverlapping(tmp.as_ptr(), rc, size);
                root = child;
            }
        };

        let n = nmemb;
        // Build max-heap.
        let mut start = n / 2;
        while start > 0 {
            start -= 1;
            sift(start, n, &mut tmp);
        }
        // Extract.
        let mut end = n;
        while end > 1 {
            end -= 1;
            let a = arr.add(0);
            let b = arr.add(end * size);
            core::ptr::copy_nonoverlapping(a, tmp.as_mut_ptr(), size);
            core::ptr::copy_nonoverlapping(b, a, size);
            core::ptr::copy_nonoverlapping(tmp.as_ptr(), b, size);
            sift(0, end, &mut tmp);
        }
    }

    // ------------------------------------------------------------------
    // strtod / strtol / strtoul
    // ------------------------------------------------------------------

    fn skip_ws(s: &[u8]) -> &[u8] {
        let mut i = 0;
        while i < s.len() && (s[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        &s[i..]
    }

    /// Parse a double following C `strtod` conventions, returning the value and
    /// the number of input bytes consumed.
    fn parse_strtod(s: &[u8]) -> (f64, usize) {
        let s0 = s;
        let s = skip_ws(s);

        let (neg, s) = match s.first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };

        // Hex float: 0x[hex][.hex]p[±]dec
        if s.len() >= 2 && s[0] == b'0' && (s[1] == b'x' || s[1] == b'X') {
            let mut i = 2;
            let mut mantissa: u64 = 0;
            let mut frac_bits: i32 = 0;
            let mut any = false;
            let mut seen_dot = false;
            while i < s.len() {
                let c = s[i];
                if let Some(d) = hex_digit(c) {
                    mantissa = mantissa.wrapping_mul(16).wrapping_add(d as u64);
                    if seen_dot {
                        frac_bits += 4;
                    }
                    any = true;
                    i += 1;
                } else if c == b'.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            let mut exp: i32 = 0;
            if any && i < s.len() && (s[i] == b'p' || s[i] == b'P') {
                let (es, eo) = parse_int_after(&s[i + 1..]);
                exp = es;
                i += 1 + eo;
            }
            if any {
                let value = mantissa as f64 * pow2_f64(exp as f64 - frac_bits as f64);
                let total = s0.len() - s.len() + i;
                return (if neg { -value } else { value }, total);
            }
            // Not a valid hex float: C consumes just "0".
            let total = s0.len() - s.len() + 1;
            return (0.0, total);
        }

        // inf / infinity / nan
        let lower: Vec<u8> = s.iter().take(8).map(|b| b.to_ascii_lowercase()).collect();
        if lower.starts_with(b"infinity") {
            let total = s0.len() - s.len() + 8;
            return (if neg { f64::NEG_INFINITY } else { f64::INFINITY }, total);
        }
        if lower.starts_with(b"inf") {
            let total = s0.len() - s.len() + 3;
            return (if neg { f64::NEG_INFINITY } else { f64::INFINITY }, total);
        }
        if lower.starts_with(b"nan") {
            let mut i = 3;
            if s.len() > i && s[i] == b'(' {
                while i < s.len() && s[i] != b')' {
                    i += 1;
                }
                if i < s.len() {
                    i += 1;
                }
            }
            let total = s0.len() - s.len() + i;
            return (f64::NAN, total);
        }

        // Decimal: scan [digits][.digits][e|E[+|-]digits]
        let mut i = 0;
        let mut any = false;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            any = true;
        }
        let mut seen_dot = false;
        if i < s.len() && s[i] == b'.' {
            seen_dot = true;
            i += 1;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
                any = true;
            }
        }
        let _ = seen_dot;
        if any && i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
                j += 1;
            }
            let ds = j;
            while j < s.len() && s[j].is_ascii_digit() {
                j += 1;
            }
            if j > ds {
                i = j;
            }
        }
        let total = s0.len() - s.len() + i;
        if !any {
            // Nothing consumed: C's endptr points at the original nptr
            // (even the sign is not consumed without digits after it).
            return (0.0, 0);
        }
        let text = &s[..i];
        let parsed: f64 = if text.is_empty() {
            0.0
        } else {
            match core::str::from_utf8(text).ok().and_then(|t| t.parse::<f64>().ok()) {
                Some(v) => v,
                None => {
                    // Last resort: strip trailing 'e'/'E' artifacts.
                    let mut v = 0.0;
                    let mut t = text;
                    while !t.is_empty() {
                        if let Ok(ts) = core::str::from_utf8(t) {
                            if let Ok(x) = ts.parse::<f64>() {
                                v = x;
                                break;
                            }
                        }
                        t = &t[..t.len() - 1];
                    }
                    v
                }
            }
        };
        (if neg { -parsed } else { parsed }, total)
    }

    fn hex_digit(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }

    fn pow2_f64(e: f64) -> f64 {
        // 2^e without relying on libm pow.
        (e as i32 as f64).exp2()
    }

    fn parse_int_after(s: &[u8]) -> (i32, usize) {
        let mut i = 0;
        let mut neg = false;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            neg = s[i] == b'-';
            i += 1;
        }
        let ds = i;
        let mut v: i32 = 0;
        while i < s.len() && s[i].is_ascii_digit() {
            v = v.saturating_mul(10).saturating_add((s[i] - b'0') as i32);
            i += 1;
        }
        if i == ds {
            return (0, 0);
        }
        (if neg { -v } else { v }, i)
    }

    #[no_mangle]
    pub unsafe extern "C" fn strtod(nptr: *const c_char, endptr: *mut *mut c_char) -> c_double {
        if nptr.is_null() {
            if !endptr.is_null() {
                *endptr = nptr as *mut c_char;
            }
            return 0.0;
        }
        let s = core::ffi::CStr::from_ptr(nptr);
        let bytes = s.to_bytes();
        let (v, consumed) = parse_strtod(bytes);
        if !endptr.is_null() {
            *endptr = nptr.add(consumed) as *mut c_char;
        }
        v
    }

    fn parse_int(s: &[u8], base: i32) -> (i128, usize, bool) {
        // Returns (value, consumed, overflow)
        let mut i = 0;
        let mut neg = false;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            neg = s[i] == b'-';
            i += 1;
        }
        let orig_base = base;
        let mut base: u32 = if orig_base == 0 { 10 } else { orig_base as u32 };
        let mut any = false;
        // auto-detect for base 0
        if orig_base == 0 {
            if i + 1 < s.len() && s[i] == b'0' && (s[i + 1] == b'x' || s[i + 1] == b'X') {
                base = 16;
                i += 2;
            } else if i < s.len() && s[i] == b'0' {
                base = 8;
            }
        } else if base == 16 && i + 1 < s.len() && s[i] == b'0'
            && (s[i + 1] == b'x' || s[i + 1] == b'X')
        {
            i += 2;
        }
        let mut v: i128 = 0;
        let mut overflow = false;
        while i < s.len() {
            let d = match hex_digit(s[i]) {
                Some(d) if (d as u32) < base => d as u32,
                _ => break,
            };
            v = v.saturating_mul(base as i128).saturating_add(d as i128);
            if !overflow && v >= (i128::MAX / 2) {
                overflow = true;
            }
            any = true;
            i += 1;
        }
        let v = if neg { -v } else { v };
        (v, i, !any || overflow)
    }

    #[no_mangle]
    pub unsafe extern "C" fn strtol(nptr: *const c_char, endptr: *mut *mut c_char, base: c_int) -> c_long {
        if nptr.is_null() {
            if !endptr.is_null() {
                *endptr = nptr as *mut c_char;
            }
            return 0;
        }
        let s = core::ffi::CStr::from_ptr(nptr).to_bytes();
        let skipped = s.len() - skip_ws(s).len();
        let (v, consumed, _ovf) = parse_int(&s[skipped..], base as i32);
        let clamped = if v > i64::MAX as i128 {
            i64::MAX
        } else if v < i64::MIN as i128 {
            i64::MIN
        } else {
            v as i64
        };
        if !endptr.is_null() {
            *endptr = nptr.add(skipped + consumed) as *mut c_char;
        }
        clamped as c_long
    }

    #[no_mangle]
    pub unsafe extern "C" fn strtoul(nptr: *const c_char, endptr: *mut *mut c_char, base: c_int) -> c_ulong {
        if nptr.is_null() {
            if !endptr.is_null() {
                *endptr = nptr as *mut c_char;
            }
            return 0;
        }
        let s = core::ffi::CStr::from_ptr(nptr).to_bytes();
        let skipped = s.len() - skip_ws(s).len();
        let (v, consumed, _ovf) = parse_int(&s[skipped..], base as i32);
        let clamped = if v < 0 {
            0u64
        } else if v > u64::MAX as i128 {
            u64::MAX
        } else {
            v as u64
        };
        if !endptr.is_null() {
            *endptr = nptr.add(skipped + consumed) as *mut c_char;
        }
        clamped as c_ulong
    }

    // ------------------------------------------------------------------
    // ctype
    // ------------------------------------------------------------------

    #[no_mangle]
    pub extern "C" fn isalpha(c: c_int) -> c_int {
        ((c as u8 as char).is_ascii_alphabetic()) as c_int
    }
    #[no_mangle]
    pub extern "C" fn isdigit(c: c_int) -> c_int {
        ((c as u8 as char).is_ascii_digit()) as c_int
    }
    #[no_mangle]
    pub extern "C" fn isalnum(c: c_int) -> c_int {
        ((c as u8 as char).is_ascii_alphanumeric()) as c_int
    }
    #[no_mangle]
    pub extern "C" fn isspace(c: c_int) -> c_int {
        ((c as u8 as char).is_ascii_whitespace()) as c_int
    }
    #[no_mangle]
    pub extern "C" fn isupper(c: c_int) -> c_int {
        ((c as u8 as char).is_ascii_uppercase()) as c_int
    }
    #[no_mangle]
    pub extern "C" fn islower(c: c_int) -> c_int {
        ((c as u8 as char).is_ascii_lowercase()) as c_int
    }
    #[no_mangle]
    pub extern "C" fn isxdigit(c: c_int) -> c_int {
        ((c as u8 as char).is_ascii_hexdigit()) as c_int
    }
    #[no_mangle]
    pub extern "C" fn iscntrl(c: c_int) -> c_int {
        let u = c as u8 as u32;
        (u < 0x20 || u == 0x7f) as c_int
    }
    #[no_mangle]
    pub extern "C" fn ispunct(c: c_int) -> c_int {
        let u = c as u8 as char;
        (u.is_ascii_punctuation()) as c_int
    }
    #[no_mangle]
    pub extern "C" fn isgraph(c: c_int) -> c_int {
        let u = c as u8 as u32;
        (u > 0x20 && u < 0x7f) as c_int
    }
    #[no_mangle]
    pub extern "C" fn isprint(c: c_int) -> c_int {
        let u = c as u8 as u32;
        (u >= 0x20 && u < 0x7f) as c_int
    }
    #[no_mangle]
    pub extern "C" fn tolower(c: c_int) -> c_int {
        (c as u8 as char).to_ascii_lowercase() as c_int
    }
    #[no_mangle]
    pub extern "C" fn toupper(c: c_int) -> c_int {
        (c as u8 as char).to_ascii_uppercase() as c_int
    }

    // ------------------------------------------------------------------
    // string.h
    // ------------------------------------------------------------------

    #[no_mangle]
    pub unsafe extern "C" fn strlen(s: *const c_char) -> usize {
        // Manual byte scan: must NOT go through CStr::from_ptr, which calls
        // the `strlen` symbol (== this function) and would recurse forever.
        let mut p = s;
        while *p != 0 {
            p = p.add(1);
        }
        (p as usize).wrapping_sub(s as usize)
    }

    #[no_mangle]
    pub unsafe extern "C" fn strcmp(s1: *const c_char, s2: *const c_char) -> c_int {
        let a = core::ffi::CStr::from_ptr(s1).to_bytes();
        let b = core::ffi::CStr::from_ptr(s2).to_bytes();
        match a.cmp(b) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn strcoll(s1: *const c_char, s2: *const c_char) -> c_int {
        // No locales on bare wasm: strcoll == strcmp.
        strcmp(s1, s2)
    }

    #[no_mangle]
    pub unsafe extern "C" fn strncmp(s1: *const c_char, s2: *const c_char, n: usize) -> c_int {
        if n == 0 {
            return 0;
        }
        let a = core::slice::from_raw_parts(s1 as *const u8, n);
        let b = core::slice::from_raw_parts(s2 as *const u8, n);
        for (x, y) in a.iter().zip(b.iter()) {
            if *x != *y {
                return (*x as c_int) - (*y as c_int);
            }
            if *x == 0 {
                break;
            }
        }
        0
    }

    #[no_mangle]
    pub unsafe extern "C" fn strchr(s: *const c_char, c: c_int) -> *mut c_char {
        let target = c as u8 as u8;
        let mut p = s;
        loop {
            if *p as u8 == target {
                return p as *mut c_char;
            }
            if *p == 0 {
                return core::ptr::null_mut();
            }
            p = p.add(1);
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn strrchr(s: *const c_char, c: c_int) -> *mut c_char {
        let bytes = core::ffi::CStr::from_ptr(s).to_bytes();
        let target = c as u8;
        if let Some(pos) = bytes.iter().rposition(|&b| b == target) {
            s.add(pos) as *mut c_char
        } else {
            core::ptr::null_mut()
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn strstr(haystack: *const c_char, needle: *const c_char) -> *mut c_char {
        let h = core::ffi::CStr::from_ptr(haystack).to_bytes();
        let n = core::ffi::CStr::from_ptr(needle).to_bytes();
        if n.is_empty() {
            return haystack as *mut c_char;
        }
        if n.len() > h.len() {
            return core::ptr::null_mut();
        }
        for (i, win) in h.windows(n.len()).enumerate() {
            if win == n {
                return haystack.add(i) as *mut c_char;
            }
        }
        core::ptr::null_mut()
    }

    #[no_mangle]
    pub unsafe extern "C" fn strpbrk(s: *const c_char, accept: *const c_char) -> *mut c_char {
        let acc = core::ffi::CStr::from_ptr(accept).to_bytes();
        let bytes = core::ffi::CStr::from_ptr(s).to_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if acc.contains(&b) {
                return s.add(i) as *mut c_char;
            }
        }
        core::ptr::null_mut()
    }

    #[no_mangle]
    pub unsafe extern "C" fn memchr(s: *const c_void, c: c_int, n: usize) -> *mut c_void {
        let bytes = core::slice::from_raw_parts(s as *const u8, n);
        if let Some(pos) = bytes.iter().position(|&b| b == c as u8) {
            (s as *mut u8).add(pos) as *mut c_void
        } else {
            core::ptr::null_mut()
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn strspn(s: *const c_char, accept: *const c_char) -> usize {
        let acc = core::ffi::CStr::from_ptr(accept).to_bytes();
        let bytes = core::ffi::CStr::from_ptr(s).to_bytes();
        let mut n = 0;
        while n < bytes.len() && acc.contains(&bytes[n]) {
            n += 1;
        }
        n
    }

    #[no_mangle]
    pub unsafe extern "C" fn strcspn(s: *const c_char, reject: *const c_char) -> usize {
        let rej = core::ffi::CStr::from_ptr(reject).to_bytes();
        let bytes = core::ffi::CStr::from_ptr(s).to_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if rej.contains(&b) {
                return i;
            }
        }
        bytes.len()
    }

    #[no_mangle]
    pub unsafe extern "C" fn strcpy(dest: *mut c_char, src: *const c_char) -> *mut c_char {
        let bytes = core::ffi::CStr::from_ptr(src).to_bytes();
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest as *mut u8, bytes.len() + 1);
        dest
    }

    #[no_mangle]
    pub unsafe extern "C" fn strncpy(dest: *mut c_char, src: *const c_char, n: usize) -> *mut c_char {
        let bytes = core::ffi::CStr::from_ptr(src).to_bytes();
        let copy = bytes.len().min(n);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest as *mut u8, copy);
        // C pads with NULs up to n.
        if copy < n {
            core::ptr::write_bytes(dest.add(copy), 0, n - copy);
        }
        dest
    }

    #[no_mangle]
    pub unsafe extern "C" fn strcat(dest: *mut c_char, src: *const c_char) -> *mut c_char {
        let len = strlen(dest);
        strcpy(dest.add(len), src);
        dest
    }

    #[no_mangle]
    pub unsafe extern "C" fn strncat(dest: *mut c_char, src: *const c_char, n: usize) -> *mut c_char {
        let dlen = strlen(dest);
        let bytes = core::ffi::CStr::from_ptr(src).to_bytes();
        let copy = bytes.len().min(n);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest.add(dlen) as *mut u8, copy);
        *dest.add(dlen + copy) = 0;
        dest
    }

    // ------------------------------------------------------------------
    // stdlib extras
    // ------------------------------------------------------------------

    #[no_mangle]
    pub unsafe extern "C" fn atoi(nptr: *const c_char) -> c_int {
        strtol(nptr, core::ptr::null_mut(), 10) as c_int
    }

    #[no_mangle]
    pub unsafe extern "C" fn atof(nptr: *const c_char) -> c_double {
        strtod(nptr, core::ptr::null_mut())
    }

    #[no_mangle]
    pub extern "C" fn abs(j: c_int) -> c_int {
        j.wrapping_abs()
    }

    #[no_mangle]
    pub extern "C" fn getenv(_name: *const c_char) -> *mut c_char {
        core::ptr::null_mut()
    }

    #[no_mangle]
    pub extern "C" fn system(_command: *const c_char) -> c_int {
        -1
    }

    // ------------------------------------------------------------------
    // time / locale stubs (os library)
    // ------------------------------------------------------------------

    #[no_mangle]
    pub extern "C" fn time(tloc: *mut c_long) -> c_long {
        // No clock on bare wasm: return -1 (os.time fails gracefully).
        if !tloc.is_null() {
            unsafe { *tloc = -1 };
        }
        -1
    }

    #[no_mangle]
    pub extern "C" fn clock() -> c_long {
        0
    }

    #[no_mangle]
    pub extern "C" fn difftime(time1: c_long, time0: c_long) -> c_double {
        (time1 - time0) as c_double
    }

    static mut TM_ZERO: TmShim = TmShim::ZERO;

    #[repr(C)]
    struct TmShim {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
    }
    impl TmShim {
        const ZERO: TmShim = TmShim {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
        };
    }

    #[no_mangle]
    pub extern "C" fn gmtime(_timep: *const c_long) -> *mut TmShim {
        core::ptr::addr_of_mut!(TM_ZERO)
    }

    #[no_mangle]
    pub extern "C" fn localtime(_timep: *const c_long) -> *mut TmShim {
        core::ptr::addr_of_mut!(TM_ZERO)
    }

    #[no_mangle]
    pub extern "C" fn mktime(_tm: *mut TmShim) -> c_long {
        -1
    }

    #[no_mangle]
    pub extern "C" fn strftime(
        _s: *mut c_char,
        _max: usize,
        _format: *const c_char,
        _tm: *const TmShim,
    ) -> usize {
        0
    }

    #[no_mangle]
    pub extern "C" fn tmpnam(_s: *mut c_char) -> *mut c_char {
        core::ptr::null_mut()
    }

    static mut LCONV: LconvShim = LconvShim::C;

    #[repr(C)]
    #[allow(dead_code)]
    struct LconvShim {
        decimal_point: *mut c_char,
        thousands_sep: *mut c_char,
        grouping: *mut c_char,
        int_curr_symbol: *mut c_char,
        currency_symbol: *mut c_char,
        mon_decimal_point: *mut c_char,
        mon_thousands_sep: *mut c_char,
        mon_grouping: *mut c_char,
        positive_sign: *mut c_char,
        negative_sign: *mut c_char,
        int_frac_digits: c_char,
        frac_digits: c_char,
        p_cs_precedes: c_char,
        p_sep_by_space: c_char,
        n_cs_precedes: c_char,
        n_sep_by_space: c_char,
        p_sign_posn: c_char,
        n_sign_posn: c_char,
        int_p_cs_precedes: c_char,
        int_p_sep_by_space: c_char,
        int_n_cs_precedes: c_char,
        int_n_sep_by_space: c_char,
        int_p_sign_posn: c_char,
        int_n_sign_posn: c_char,
    }
    impl LconvShim {
        const C: LconvShim = LconvShim {
            decimal_point: b".\0".as_ptr() as *mut c_char,
            thousands_sep: b"\0".as_ptr() as *mut c_char,
            grouping: b"\0".as_ptr() as *mut c_char,
            int_curr_symbol: b"\0".as_ptr() as *mut c_char,
            currency_symbol: b"\0".as_ptr() as *mut c_char,
            mon_decimal_point: b"\0".as_ptr() as *mut c_char,
            mon_thousands_sep: b"\0".as_ptr() as *mut c_char,
            mon_grouping: b"\0".as_ptr() as *mut c_char,
            positive_sign: b"\0".as_ptr() as *mut c_char,
            negative_sign: b"\0".as_ptr() as *mut c_char,
            int_frac_digits: 0,
            frac_digits: 0,
            p_cs_precedes: 0,
            p_sep_by_space: 0,
            n_cs_precedes: 0,
            n_sep_by_space: 0,
            p_sign_posn: 0,
            n_sign_posn: 0,
            int_p_cs_precedes: 0,
            int_p_sep_by_space: 0,
            int_n_cs_precedes: 0,
            int_n_sep_by_space: 0,
            int_p_sign_posn: 0,
            int_n_sign_posn: 0,
        };
    }

    #[no_mangle]
    pub extern "C" fn localeconv() -> *mut LconvShim {
        core::ptr::addr_of_mut!(LCONV)
    }

    #[no_mangle]
    pub extern "C" fn setlocale(_category: c_int, _locale: *const c_char) -> *mut c_char {
        core::ptr::null_mut()
    }

    // ------------------------------------------------------------------
    // libm exports (provided by the `libm` crate, which mirrors musl)
    // ------------------------------------------------------------------

    macro_rules! libm_export {
        ($rust:ident, $cname:ident, $($arg:ident : $ty:ty),+) => {
            #[no_mangle]
            pub extern "C" fn $cname($($arg: $ty),+) -> c_double {
                libm::$rust($($arg),+)
            }
        };
    }

    libm_export!(sin, sin, x: c_double);
    libm_export!(cos, cos, x: c_double);
    libm_export!(tan, tan, x: c_double);
    libm_export!(asin, asin, x: c_double);
    libm_export!(acos, acos, x: c_double);
    libm_export!(atan, atan, x: c_double);
    libm_export!(atan2, atan2, y: c_double, x: c_double);
    libm_export!(sinh, sinh, x: c_double);
    libm_export!(cosh, cosh, x: c_double);
    libm_export!(tanh, tanh, x: c_double);
    libm_export!(exp, exp, x: c_double);
    libm_export!(log, log, x: c_double);
    libm_export!(log10, log10, x: c_double);
    libm_export!(log2, log2, x: c_double);
    libm_export!(exp2, exp2, x: c_double);
    libm_export!(hypot, hypot, x: c_double, y: c_double);
    libm_export!(pow, pow, x: c_double, y: c_double);
    libm_export!(fmod, fmod, x: c_double, y: c_double);
    libm_export!(sqrt, sqrt, x: c_double);
    libm_export!(floor, floor, x: c_double);
    libm_export!(ceil, ceil, x: c_double);
    libm_export!(fabs, fabs, x: c_double);
    libm_export!(round, round, x: c_double);
    libm_export!(trunc, trunc, x: c_double);

    #[no_mangle]
    pub extern "C" fn frexp(x: c_double, exp: *mut c_int) -> c_double {
        let (m, e) = libm::frexp(x);
        if !exp.is_null() {
            unsafe { *exp = e };
        }
        m
    }

    #[no_mangle]
    pub extern "C" fn ldexp(x: c_double, exp: c_int) -> c_double {
        libm::ldexp(x, exp)
    }

    #[no_mangle]
    pub extern "C" fn modf(x: c_double, iptr: *mut c_double) -> c_double {
        let i = libm::trunc(x);
        if !iptr.is_null() {
            unsafe { *iptr = i };
        }
        if x.is_infinite() {
            0.0
        } else {
            x - i
        }
    }

    // ------------------------------------------------------------------
    // Float formatting helper used by the C printf family.
    //
    // Renders `v` for conversions f/e/E/g/G with the given C precision,
    // including the sign but without any width/zero padding.
    // ------------------------------------------------------------------

    #[no_mangle]
    pub unsafe extern "C" fn __shim_double_to_str(
        buf: *mut c_char,
        cap: usize,
        v: c_double,
        conv: c_int,
        prec: c_int,
        alt: c_int,
        force_sign: c_int,
    ) -> c_int {
        let out = core::slice::from_raw_parts_mut(buf as *mut u8, cap);
        let text = format_double(v, conv as u8, prec as usize, alt != 0, force_sign);
        let len = text.len().min(cap.saturating_sub(1));
        out[..len].copy_from_slice(&text.as_bytes()[..len]);
        if cap > 0 {
            out[len] = 0;
        }
        text.len() as c_int
    }

    fn format_double(v: f64, conv: u8, prec: usize, alt: bool, force_sign: i32) -> String {
        let upper = conv == b'E' || conv == b'G';
        let base = if upper { conv } else { conv };
        let mut out = String::new();

        let neg = v.is_sign_negative() && !v.is_nan();
        let sign = if neg {
            '-'
        } else if force_sign == 1 {
            '+'
        } else if force_sign == 2 {
            ' '
        } else {
            '\0'
        };
        if sign != '\0' {
            out.push(sign);
        }

        if v.is_nan() {
            out.push_str(if upper { "NAN" } else { "nan" });
            return out;
        }
        if v.is_infinite() {
            out.push_str(if upper { "INF" } else { "inf" });
            return out;
        }

        let prec = if prec == 0 && base == b'g' { 1 } else { prec };

        match base {
            b'f' => {
                let s = format!("{:.*}", prec, v.abs());
                out.push_str(&s);
            }
            b'e' => {
                let s = format!("{:.*e}", prec, v.abs());
                out.push_str(&normalize_e(&s, upper));
            }
            b'g' => {
                out.push_str(&format_g(v.abs(), prec, alt, upper));
            }
            _ => {
                // fallback
                out.push_str(&format!("{}", v));
            }
        }
        out
    }

    fn normalize_e(s: &str, upper: bool) -> String {
        // Rust {:e}: "1.5e-5" or "1e20". Convert to C: "1.5e-05", "1e+20".
        let (mant, exp) = match s.split_once('e') {
            Some((m, e)) => (m, e),
            None => return s.to_string(),
        };
        let mut out = String::with_capacity(s.len() + 2);
        out.push_str(mant);
        out.push(if upper { 'E' } else { 'e' });
        let (neg_exp, digits) = if let Some(d) = exp.strip_prefix('-') {
            (true, d)
        } else {
            (false, exp)
        };
        if !neg_exp {
            out.push('+');
        } else {
            out.push('-');
        }
        if digits.len() < 2 {
            out.push('0');
        }
        out.push_str(digits);
        out
    }

    fn format_g(v: f64, prec: usize, alt: bool, upper: bool) -> String {
        if v == 0.0 {
            if alt {
                return "0.".to_string();
            }
            return "0".to_string();
        }
        // Round to `prec` significant digits via scientific form.
        let sci = format!("{:.*e}", prec - 1, v); // e.g. "3.14159265358979e0"
        let (mant, exp_str) = sci.split_once('e').unwrap();
        let exp: i32 = exp_str.parse().unwrap_or(0);
        // Mantissa digits without the dot: d1 d2 d3 ... (prec digits).
        let digits: String = mant.chars().filter(|c| *c != '.').collect();
        let digits = digits.as_str();

        let exp_char = if upper { 'E' } else { 'e' };
        let mut out = String::new();

        if exp < -4 || exp >= prec as i32 {
            // scientific: d[.ddd]e±XX
            out.push(digits.as_bytes()[0] as char);
            let frac = &digits[1..];
            let frac = trim_trailing_zeros(frac, alt);
            if !frac.is_empty() || alt {
                out.push('.');
                out.push_str(&frac);
            }
            out.push(exp_char);
            if exp >= 0 {
                out.push('+');
            } else {
                out.push('-');
            }
            let abs = exp.abs();
            if abs < 10 {
                out.push('0');
            }
            out.push_str(&abs.to_string());
        } else if exp >= 0 {
            // fixed with exp+1 integer digits
            let int_len = exp as usize + 1;
            let mut int_part = String::new();
            for i in 0..int_len {
                int_part.push(digits.as_bytes().get(i).copied().unwrap_or(b'0') as char);
            }
            let frac = &digits[int_len..];
            let frac = trim_trailing_zeros(frac, alt);
            out.push_str(&int_part);
            if !frac.is_empty() || alt {
                out.push('.');
                out.push_str(&frac);
            }
        } else {
            // 0.000...ddd
            out.push_str("0.");
            let zeros = (-exp - 1) as usize;
            for _ in 0..zeros {
                out.push('0');
            }
            // %.14e 把小数展开成 prec 位有效数字（0.1 -> "10000000000000"），
            // 与另两个分支一致，非 alt 形式必须去掉尾零，否则 tostring(0.1)
            // 会输出 "0.10000000000000"。
            let frac = trim_trailing_zeros(digits, alt);
            out.push_str(&frac);
        }
        out
    }

    fn trim_trailing_zeros(s: &str, alt: bool) -> String {
        if alt {
            return s.to_string();
        }
        let trimmed = s.trim_end_matches('0');
        trimmed.to_string()
    }
}
