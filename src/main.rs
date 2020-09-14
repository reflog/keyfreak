use once_cell::sync::OnceCell;
use rdev::{listen, Event, EventType, Key};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::thread;
extern crate simple_excel_writer as excel;

use excel::*;

struct Capture {
    stack: RefCell<Vec<Key>>,
    seq: RefCell<Vec<Key>>,
    data: RefCell<HashMap<String, HashMap<String, i32>>>,
}

impl Capture {
    pub fn global() -> &'static Mutex<Capture> {
        INSTANCE.get().expect("capture is not initialized")
    }
}

static INSTANCE: OnceCell<Mutex<Capture>> = OnceCell::new();

static FILENAME: &str = "capture.json";

fn get_proc_and_focused_window_pid() -> Result<String, String> {
    use byteorder::{LittleEndian, ReadBytesExt};

    let (conn, screen_num) = xcb::Connection::connect(None)
        .map_err(|error| format!("Unable to open X11 connection: {}.", error))?;

    let active_window_atom_cookie = xcb::intern_atom(&conn, false, "_NET_ACTIVE_WINDOW");
    let pid_atom_cookie = xcb::intern_atom(&conn, false, "_NET_WM_PID");

    //get window
    let root = conn
        .get_setup()
        .roots()
        .nth(screen_num as usize)
        .ok_or_else(|| "Unable to select current screen.".to_string())?
        .root();

    let active_window_atom = active_window_atom_cookie
        .get_reply()
        .map_err(|error| format!("Unable to retrieve _NET_ACTIVE_WINDOW atom: {}.", error))?
        .atom();

    let mut reply = xcb::get_property(
        &conn,
        false,
        root,
        active_window_atom,
        xcb::ATOM_WINDOW,
        0,
        1,
    )
    .get_reply()
    .map_err(|error| {
        format!(
            "Unable to retrieve _NET_ACTIVE_WINDOW property from root: {}.",
            error
        )
    })?;
    if reply.value_len() == 0 {
        return Err("Unable to retrieve _NET_ACTIVE_WINDOW property from root.".to_string());
    }
    assert_eq!(reply.value_len(), 1);
    let mut raw = reply.value();
    assert_eq!(
        raw.len(),
        4,
        "_NET_ACTIVE_WINDOW property is expected to be at least 4 bytes."
    );
    let window = raw.read_u32::<LittleEndian>().unwrap() as xcb::Window;
    if window == xcb::WINDOW_NONE {
        return Err("No window is focused".to_string());
    }

    //get pid
    let pid_atom = pid_atom_cookie
        .get_reply()
        .map_err(|error| format!("Unable to retrieve _NET_WM_PID: {}.", error))?
        .atom();

    let ureply =
        xcb::get_property(&conn, false, window, pid_atom, xcb::ATOM_CARDINAL, 0, 1).get_reply();
    if let Err(e) = ureply {
        return Err(e.to_string());
    }
    reply = ureply.unwrap();
    if reply.value_len() == 0 {
        eprintln!(
            "Unable to retrieve _NET_WM_PID from focused window {}; trying WM_CLASS.",
            window
        );
        //TODO: what's a good size here?
        let reply = xcb::get_property(
            &conn,
            false,
            window,
            xcb::ATOM_WM_CLASS,
            xcb::ATOM_STRING,
            0,
            64,
        )
        .get_reply()
        .unwrap_or_else(|error| {
            panic!(
                "Unable to retrieve WM_CLASS from focused window {}: {}",
                window, error
            )
        });
        let class = String::from_utf8(
            reply
                .value()
                .iter()
                .cloned()
                .take_while(|c| *c != 0u8)
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|error| {
            panic!("Unable to decode {:#?}: {}", reply.value() as &[u8], error)
        });
        //TODO: find processes named 'class', compare cwds
        return Err(format!("Unimplemented: Find processes named {}", class));
    }
    assert_eq!(reply.value_len(), 1);
    let mut raw = reply.value();
    assert_eq!(
        raw.len(),
        4,
        "_NET_WM_PID property is expected to be at least 4 bytes"
    );

    //open proc
    let pid = raw.read_u32::<LittleEndian>().unwrap();
    let proc = std::fs::read_to_string(format!("/proc/{}/comm", pid)).map_err(|e| e.to_string())?;

    Ok(proc.trim().to_string())
}

fn sheet_writer(wb: &mut Workbook, name: &str, d: &HashMap<String, i32>) {
    let mut sheet = wb.create_sheet(name);
    sheet.add_column(excel::Column { width: 80.0 });
    sheet.add_column(excel::Column { width: 20.0 });
    wb.write_sheet(&mut sheet, |sheet_writer| {
        let sw = sheet_writer;
        sw.append_row(row!["Name", "Frequency"])
            .expect("Cannot add row");
        for (key_name, freq) in d {
            sw.append_row(row![key_name.to_string(), freq.to_string()])
                .expect("Cannot add row");
        }
        Ok(())
    })
    .expect("write excel error!");
}

fn export() {
    let mut wb = Workbook::create("report.xlsx");
    let mut c = Capture::global().lock().unwrap();
    let data = c.data.get_mut();
    let mut total: HashMap<String, i32> = HashMap::new();
    for (_, entries) in data.iter() {
        for (k, v) in entries {
            *total.entry(k.to_string()).or_default() += v;
        }
    }

    sheet_writer(&mut wb, "Total", &total);

    for key in data.keys() {
        sheet_writer(&mut wb, key, data.get(key).unwrap());
    }
    wb.close().expect("close excel error!");
}

fn handle_event(e: Event) {
    let c = Capture::global().lock().unwrap();
    let mut seq = c.seq.borrow_mut();
    let mut stack = c.stack.borrow_mut();
    let mut data = c.data.borrow_mut();
    match e.event_type {
        EventType::KeyPress(k) => {
            stack.push(k);
        }
        EventType::KeyRelease(_k) => {
            if let Ok(w) = get_proc_and_focused_window_pid() {
                if stack.is_empty() {
                    return;
                }
                let stack_last = stack.pop().unwrap();
                seq.push(stack_last);
                if stack.is_empty() {
                    let mut r = seq.clone();
                    r.reverse();

                    let cap = format!("{:?}", r);
                    debug!("captured {:?} for {:?}", cap, w);
                    *data.entry(w).or_default().entry(cap).or_default() += 1;
                    seq.clear();
                }
            }
        }
        _ => (),
    }
}

fn setup() {
    INSTANCE
        .set(Mutex::new(Capture {
            stack: RefCell::new(vec![]),
            seq: RefCell::new(vec![]),
            data: RefCell::new(HashMap::new()),
        }))
        .unwrap_or(());
}

fn restore_dump() {
    if std::path::Path::new(FILENAME).exists() {
        let data_str = std::fs::read_to_string(FILENAME).unwrap();
        let parsed: HashMap<String, HashMap<String, i32>> =
            serde_json::from_str(&data_str).unwrap();
        Capture::global()
            .lock()
            .unwrap()
            .data
            .borrow_mut()
            .extend(parsed);
    }
}

fn capture() {
    thread::spawn(move || loop {
        loop {
            thread::sleep(std::time::Duration::from_secs(10));
            let c = Capture::global().lock().unwrap();
            let data = c.data.borrow_mut();
            let data_str = format!("{:?}", data);
            std::fs::write(FILENAME, data_str).expect("Cannot write log");
            debug!("dumping, entries: {:?}", data.len());
        }
    });
    info!("Starting capture");
    if let Err(error) = listen(handle_event) {
        error!("Error: {:?}", error)
    }
}

#[macro_use]
extern crate log;
use simple_logger::SimpleLogger;

fn main() {
    SimpleLogger::new().init().unwrap();

    setup();
    restore_dump();
    if std::env::args().any(|i| i == "--export") {
        export();
        return;
    }
    capture();
}
