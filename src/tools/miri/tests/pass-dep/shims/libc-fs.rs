//@ignore-target-windows: no libc on Windows
//@compile-flags: -Zmiri-disable-isolation

#![feature(io_error_more)]
#![feature(io_error_uncategorized)]

use std::ffi::{CStr, CString, OsString};
use std::fs::{canonicalize, remove_dir_all, remove_file, File};
use std::io::{Error, ErrorKind, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

#[path = "../../utils/mod.rs"]
mod utils;

fn main() {
    test_dup_stdout_stderr();
    test_canonicalize_too_long();
    test_rename();
    test_ftruncate::<libc::off_t>(libc::ftruncate);
    #[cfg(target_os = "linux")]
    test_ftruncate::<libc::off64_t>(libc::ftruncate64);
    test_readlink();
    test_file_open_unix_allow_two_args();
    test_file_open_unix_needs_three_args();
    test_file_open_unix_extra_third_arg();
    #[cfg(target_os = "linux")]
    test_o_tmpfile_flag();
    test_posix_mkstemp();
    test_posix_realpath_alloc();
    test_posix_realpath_noalloc();
    test_posix_realpath_errors();
    #[cfg(target_os = "linux")]
    test_posix_fadvise();
    #[cfg(target_os = "linux")]
    test_sync_file_range();
    test_isatty();
}

/// Prepare: compute filename and make sure the file does not exist.
fn prepare(filename: &str) -> PathBuf {
    let path = utils::tmp().join(filename);
    // Clean the paths for robustness.
    remove_file(&path).ok();
    path
}

/// Prepare directory: compute directory name and make sure it does not exist.
#[allow(unused)]
fn prepare_dir(dirname: &str) -> PathBuf {
    let path = utils::tmp().join(&dirname);
    // Clean the directory for robustness.
    remove_dir_all(&path).ok();
    path
}

/// Prepare like above, and also write some initial content to the file.
fn prepare_with_content(filename: &str, content: &[u8]) -> PathBuf {
    let path = prepare(filename);
    let mut file = File::create(&path).unwrap();
    file.write(content).unwrap();
    path
}

fn test_file_open_unix_allow_two_args() {
    let path = prepare_with_content("test_file_open_unix_allow_two_args.txt", &[]);

    let mut name = path.into_os_string();
    name.push("\0");
    let name_ptr = name.as_bytes().as_ptr().cast::<libc::c_char>();
    let _fd = unsafe { libc::open(name_ptr, libc::O_RDONLY) };
}

fn test_file_open_unix_needs_three_args() {
    let path = prepare_with_content("test_file_open_unix_needs_three_args.txt", &[]);

    let mut name = path.into_os_string();
    name.push("\0");
    let name_ptr = name.as_bytes().as_ptr().cast::<libc::c_char>();
    let _fd = unsafe { libc::open(name_ptr, libc::O_CREAT, 0o666) };
}

fn test_file_open_unix_extra_third_arg() {
    let path = prepare_with_content("test_file_open_unix_extra_third_arg.txt", &[]);

    let mut name = path.into_os_string();
    name.push("\0");
    let name_ptr = name.as_bytes().as_ptr().cast::<libc::c_char>();
    let _fd = unsafe { libc::open(name_ptr, libc::O_RDONLY, 42) };
}

fn test_dup_stdout_stderr() {
    let bytes = b"hello dup fd\n";
    unsafe {
        let new_stdout = libc::fcntl(1, libc::F_DUPFD, 0);
        let new_stderr = libc::fcntl(2, libc::F_DUPFD, 0);
        libc::write(new_stdout, bytes.as_ptr() as *const libc::c_void, bytes.len());
        libc::write(new_stderr, bytes.as_ptr() as *const libc::c_void, bytes.len());
    }
}

fn test_canonicalize_too_long() {
    // Make sure we get an error for long paths.
    let too_long = "x/".repeat(libc::PATH_MAX.try_into().unwrap());
    assert!(canonicalize(too_long).is_err());
}

fn test_readlink() {
    let bytes = b"Hello, World!\n";
    let path = prepare_with_content("miri_test_fs_link_target.txt", bytes);
    let expected_path = path.as_os_str().as_bytes();

    let symlink_path = prepare("miri_test_fs_symlink.txt");
    std::os::unix::fs::symlink(&path, &symlink_path).unwrap();

    // Test that the expected string gets written to a buffer of proper
    // length, and that a trailing null byte is not written.
    let symlink_c_str = CString::new(symlink_path.as_os_str().as_bytes()).unwrap();
    let symlink_c_ptr = symlink_c_str.as_ptr();

    // Make the buf one byte larger than it needs to be,
    // and check that the last byte is not overwritten.
    let mut large_buf = vec![0xFF; expected_path.len() + 1];
    let res =
        unsafe { libc::readlink(symlink_c_ptr, large_buf.as_mut_ptr().cast(), large_buf.len()) };
    // Check that the resolved path was properly written into the buf.
    assert_eq!(&large_buf[..(large_buf.len() - 1)], expected_path);
    assert_eq!(large_buf.last(), Some(&0xFF));
    assert_eq!(res, large_buf.len() as isize - 1);

    // Test that the resolved path is truncated if the provided buffer
    // is too small.
    let mut small_buf = [0u8; 2];
    let res =
        unsafe { libc::readlink(symlink_c_ptr, small_buf.as_mut_ptr().cast(), small_buf.len()) };
    assert_eq!(small_buf, &expected_path[..small_buf.len()]);
    assert_eq!(res, small_buf.len() as isize);

    // Test that we report a proper error for a missing path.
    let bad_path = CString::new("MIRI_MISSING_FILE_NAME").unwrap();
    let res = unsafe {
        libc::readlink(bad_path.as_ptr(), small_buf.as_mut_ptr().cast(), small_buf.len())
    };
    assert_eq!(res, -1);
    assert_eq!(Error::last_os_error().kind(), ErrorKind::NotFound);
}

fn test_rename() {
    let path1 = prepare("miri_test_libc_fs_source.txt");
    let path2 = prepare("miri_test_libc_fs_rename_destination.txt");

    let file = File::create(&path1).unwrap();
    drop(file);

    let c_path1 = CString::new(path1.as_os_str().as_bytes()).expect("CString::new failed");
    let c_path2 = CString::new(path2.as_os_str().as_bytes()).expect("CString::new failed");

    // Renaming should succeed
    unsafe { libc::rename(c_path1.as_ptr(), c_path2.as_ptr()) };
    // Check that old file path isn't present
    assert_eq!(ErrorKind::NotFound, path1.metadata().unwrap_err().kind());
    // Check that the file has moved successfully
    assert!(path2.metadata().unwrap().is_file());

    // Renaming a nonexistent file should fail
    let res = unsafe { libc::rename(c_path1.as_ptr(), c_path2.as_ptr()) };
    assert_eq!(res, -1);
    assert_eq!(Error::last_os_error().kind(), ErrorKind::NotFound);

    remove_file(&path2).unwrap();
}

fn test_ftruncate<T: From<i32>>(
    ftruncate: unsafe extern "C" fn(fd: libc::c_int, length: T) -> libc::c_int,
) {
    // libc::off_t is i32 in target i686-unknown-linux-gnu
    // https://docs.rs/libc/latest/i686-unknown-linux-gnu/libc/type.off_t.html

    let bytes = b"hello";
    let path = prepare("miri_test_libc_fs_ftruncate.txt");
    let mut file = File::create(&path).unwrap();
    file.write(bytes).unwrap();
    file.sync_all().unwrap();
    assert_eq!(file.metadata().unwrap().len(), 5);

    let c_path = CString::new(path.as_os_str().as_bytes()).expect("CString::new failed");
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR) };

    // Truncate to a bigger size
    let mut res = unsafe { ftruncate(fd, T::from(10)) };
    assert_eq!(res, 0);
    assert_eq!(file.metadata().unwrap().len(), 10);

    // Write after truncate
    file.write(b"dup").unwrap();
    file.sync_all().unwrap();
    assert_eq!(file.metadata().unwrap().len(), 10);

    // Truncate to smaller size
    res = unsafe { ftruncate(fd, T::from(2)) };
    assert_eq!(res, 0);
    assert_eq!(file.metadata().unwrap().len(), 2);

    remove_file(&path).unwrap();
}

#[cfg(target_os = "linux")]
fn test_o_tmpfile_flag() {
    use std::fs::{create_dir, OpenOptions};
    use std::os::unix::fs::OpenOptionsExt;
    let dir_path = prepare_dir("miri_test_fs_dir");
    create_dir(&dir_path).unwrap();
    // test that the `O_TMPFILE` custom flag gracefully errors instead of stopping execution
    assert_eq!(
        Some(libc::EOPNOTSUPP),
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_TMPFILE)
            .open(dir_path)
            .unwrap_err()
            .raw_os_error(),
    );
}

fn test_posix_mkstemp() {
    use std::ffi::OsStr;
    use std::os::unix::io::FromRawFd;
    use std::path::Path;

    let valid_template = "fooXXXXXX";
    // C needs to own this as `mkstemp(3)` says:
    // "Since it will be modified, `template` must not be a string constant, but
    // should be declared as a character array."
    // There seems to be no `as_mut_ptr` on `CString` so we need to use `into_raw`.
    let ptr = CString::new(valid_template).unwrap().into_raw();
    let fd = unsafe { libc::mkstemp(ptr) };
    // Take ownership back in Rust to not leak memory.
    let slice = unsafe { CString::from_raw(ptr) };
    assert!(fd > 0);
    let osstr = OsStr::from_bytes(slice.to_bytes());
    let path: &Path = osstr.as_ref();
    let name = path.file_name().unwrap().to_string_lossy();
    assert!(name.ne("fooXXXXXX"));
    assert!(name.starts_with("foo"));
    assert_eq!(name.len(), 9);
    assert_eq!(
        name.chars().skip(3).filter(char::is_ascii_alphanumeric).collect::<Vec<char>>().len(),
        6
    );
    let file = unsafe { File::from_raw_fd(fd) };
    assert!(file.set_len(0).is_ok());

    let invalid_templates = vec!["foo", "barXX", "XXXXXXbaz", "whatXXXXXXever", "X"];
    for t in invalid_templates {
        let ptr = CString::new(t).unwrap().into_raw();
        let fd = unsafe { libc::mkstemp(ptr) };
        let _ = unsafe { CString::from_raw(ptr) };
        // "On error, -1 is returned, and errno is set to
        // indicate the error"
        assert_eq!(fd, -1);
        let e = std::io::Error::last_os_error();
        assert_eq!(e.raw_os_error(), Some(libc::EINVAL));
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
    }
}

/// Test allocating variant of `realpath`.
fn test_posix_realpath_alloc() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::ffi::OsStringExt;

    let buf;
    let path = utils::tmp().join("miri_test_libc_posix_realpath_alloc");
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("CString::new failed");

    // Cleanup before test.
    remove_file(&path).ok();
    // Create file.
    drop(File::create(&path).unwrap());
    unsafe {
        let r = libc::realpath(c_path.as_ptr(), std::ptr::null_mut());
        assert!(!r.is_null());
        buf = CStr::from_ptr(r).to_bytes().to_vec();
        libc::free(r as *mut _);
    }
    let canonical = PathBuf::from(OsString::from_vec(buf));
    assert_eq!(path.file_name(), canonical.file_name());

    // Cleanup after test.
    remove_file(&path).unwrap();
}

/// Test non-allocating variant of `realpath`.
fn test_posix_realpath_noalloc() {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let path = utils::tmp().join("miri_test_libc_posix_realpath_noalloc");
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("CString::new failed");

    let mut v = vec![0; libc::PATH_MAX as usize];

    // Cleanup before test.
    remove_file(&path).ok();
    // Create file.
    drop(File::create(&path).unwrap());
    unsafe {
        let r = libc::realpath(c_path.as_ptr(), v.as_mut_ptr());
        assert!(!r.is_null());
    }
    let c = unsafe { CStr::from_ptr(v.as_ptr()) };
    let canonical = PathBuf::from(c.to_str().expect("CStr to str"));

    assert_eq!(path.file_name(), canonical.file_name());

    // Cleanup after test.
    remove_file(&path).unwrap();
}

/// Test failure cases for `realpath`.
fn test_posix_realpath_errors() {
    use std::ffi::CString;
    use std::io::ErrorKind;

    // Test nonexistent path returns an error.
    let c_path = CString::new("./nothing_to_see_here").expect("CString::new failed");
    let r = unsafe { libc::realpath(c_path.as_ptr(), std::ptr::null_mut()) };
    assert!(r.is_null());
    let e = std::io::Error::last_os_error();
    assert_eq!(e.raw_os_error(), Some(libc::ENOENT));
    assert_eq!(e.kind(), ErrorKind::NotFound);
}

#[cfg(target_os = "linux")]
fn test_posix_fadvise() {
    use std::io::Write;

    let path = utils::tmp().join("miri_test_libc_posix_fadvise.txt");
    // Cleanup before test
    remove_file(&path).ok();

    // Set up an open file
    let mut file = File::create(&path).unwrap();
    let bytes = b"Hello, World!\n";
    file.write(bytes).unwrap();

    // Test calling posix_fadvise on a file.
    let result = unsafe {
        libc::posix_fadvise(
            file.as_raw_fd(),
            0,
            bytes.len().try_into().unwrap(),
            libc::POSIX_FADV_DONTNEED,
        )
    };
    drop(file);
    remove_file(&path).unwrap();
    assert_eq!(result, 0);
}

#[cfg(target_os = "linux")]
fn test_sync_file_range() {
    use std::io::Write;

    let path = utils::tmp().join("miri_test_libc_sync_file_range.txt");
    // Cleanup before test.
    remove_file(&path).ok();

    // Write to a file.
    let mut file = File::create(&path).unwrap();
    let bytes = b"Hello, World!\n";
    file.write(bytes).unwrap();

    // Test calling sync_file_range on the file.
    let result_1 = unsafe {
        libc::sync_file_range(
            file.as_raw_fd(),
            0,
            0,
            libc::SYNC_FILE_RANGE_WAIT_BEFORE
                | libc::SYNC_FILE_RANGE_WRITE
                | libc::SYNC_FILE_RANGE_WAIT_AFTER,
        )
    };
    drop(file);

    // Test calling sync_file_range on a file opened for reading.
    let file = File::open(&path).unwrap();
    let result_2 = unsafe {
        libc::sync_file_range(
            file.as_raw_fd(),
            0,
            0,
            libc::SYNC_FILE_RANGE_WAIT_BEFORE
                | libc::SYNC_FILE_RANGE_WRITE
                | libc::SYNC_FILE_RANGE_WAIT_AFTER,
        )
    };
    drop(file);

    remove_file(&path).unwrap();
    assert_eq!(result_1, 0);
    assert_eq!(result_2, 0);
}

fn test_isatty() {
    // Testing whether our isatty shim returns the right value would require controlling whether
    // these streams are actually TTYs, which is hard.
    // For now, we just check that these calls are supported at all.
    unsafe {
        libc::isatty(libc::STDIN_FILENO);
        libc::isatty(libc::STDOUT_FILENO);
        libc::isatty(libc::STDERR_FILENO);

        // But when we open a file, it is definitely not a TTY.
        let path = utils::tmp().join("notatty.txt");
        // Cleanup before test.
        remove_file(&path).ok();
        let file = File::create(&path).unwrap();

        assert_eq!(libc::isatty(file.as_raw_fd()), 0);
        assert_eq!(std::io::Error::last_os_error().raw_os_error().unwrap(), libc::ENOTTY);

        // Cleanup after test.
        drop(file);
        remove_file(&path).unwrap();
    }
}
