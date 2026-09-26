//! Захват настоящего окна. Тест рисует своё окно — левая половина красная,
//! правая синяя — и захватывает его без системного выбора. Выключен по
//! умолчанию: нужен рабочий стол Windows (у CI его нет).
//!
//! `cargo test -p analyzer-capture -- --ignored`

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use analyzer_vision::{Frame, FrameSlot};
use windows::Graphics::Capture::GraphicsCaptureItem;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, PAINTSTRUCT,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    SetWindowPos, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_PAINT, WNDCLASSW, WS_POPUP,
    WS_VISIBLE,
};
use windows::core::w;

use super::CaptureSession;
use super::source::source;
use crate::{CaptureConfig, RegionF};

const RED: [u8; 4] = [0, 0, 255, 255];
const BLUE: [u8; 4] = [255, 0, 0, 255];

#[test]
#[ignore = "нужен рабочий стол Windows"]
fn a_real_window_is_captured_cropped_resent_and_followed_as_it_grows() {
    captured_cropped_resent_and_followed_as_it_grows();
}

/// Захват начинает главный поток приложения, а он — в однопоточном
/// подразделении COM (STA): так его настраивает GPUI. Кадры же Windows
/// присылает из своего пула потоков, и окно, развёрнутое во весь экран,
/// должно захватываться и дальше.
#[test]
#[ignore = "нужен рабочий стол Windows"]
fn a_capture_started_on_the_ui_thread_follows_a_window_as_it_grows() {
    // SAFETY: поток теста входит в STA один раз и до конца теста.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok().unwrap();
    captured_cropped_resent_and_followed_as_it_grows();
}

fn captured_cropped_resent_and_followed_as_it_grows() {
    // Зависание — тоже провал, и понятнее, чем тест, который не кончается.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(60));
        eprintln!("capture test hung for a minute");
        std::process::abort();
    });
    let window = TestWindow::open();
    // Окно выбирается так же, как его отдаёт системный выбор, — в потоке из
    // пула Windows, то есть в многопоточном подразделении COM.
    let hwnd = window.hwnd;
    let picked = std::thread::spawn(move || {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().unwrap();
        // SAFETY: окно живо, его поток ждёт сообщений.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(HWND(hwnd as *mut _)) }.unwrap();
        source(&item).unwrap()
    })
    .join()
    .unwrap();

    let setup = Arc::new(FrameSlot::new());
    let config = CaptureConfig { fps: 10, region: None, max_side_region: 64, max_side_full: 1600 };
    let capture = CaptureSession::start(picked, config, Arc::clone(&setup), |_| {}).unwrap();

    // Окно целиком — в полный размер (при масштабе экрана больше 100 % оно
    // больше 400×300), слева красное, справа синее.
    let whole = frame_where(&setup, halves_are_red_and_blue);
    assert!(whole.width() >= 400 && whole.height() >= 300, "{whole:?}");

    // Правая половина, уменьшенная до 64 точек по длинной стороне. Окно
    // неподвижно, и Windows новых кадров не шлёт — кадр приходит только
    // потому, что захват отдаёт последний заново.
    let right = RegionF { x: 0.5, y: 0.0, width: 0.5, height: 1.0 };
    capture.reconfigure(CaptureConfig { region: Some(right), ..config }).unwrap();
    let board = frame_where(&setup, |frame| frame.width().max(frame.height()) == 64);
    assert!(close_to(board.pixel(board.width() / 2, board.height() / 2), BLUE), "{board:?}");
    let expected_width = f64::from(whole.width()) / 2.0 * 64.0 / f64::from(whole.height());
    assert!((f64::from(board.width()) - expected_width).abs() <= 1.0, "{board:?}");

    // Новый получатель сразу получает последний кадр.
    let session = Arc::new(FrameSlot::new());
    capture.set_target(Arc::clone(&session));
    let resent = frame_where(&session, |_| true);
    assert_eq!((resent.width(), resent.height()), (board.width(), board.height()));

    // Окно выросло больше буферов захвата: захват растёт вместе с ним.
    capture.set_region(None).unwrap();
    window.resize(600, 450);
    let grown =
        frame_where(&session, |frame| frame.width() > whole.width() && halves_are_red_and_blue(frame));
    assert!(grown.height() > whole.height(), "{grown:?}");

    drop(capture);
}

fn halves_are_red_and_blue(frame: &Frame) -> bool {
    let (width, height) = (frame.width(), frame.height());
    close_to(frame.pixel(width / 4, height / 2), RED)
        && close_to(frame.pixel(width * 3 / 4, height / 2), BLUE)
}

/// Ждёт кадр, для которого `wanted` истинно. Паникует, если за 5 секунд
/// такого нет.
fn frame_where(slot: &FrameSlot, wanted: impl Fn(&Frame) -> bool) -> Frame {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut seen, mut last) = (0, None);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Some((seq, frame)) = slot.wait_newer(seen, left) {
            if wanted(&frame) {
                return frame;
            }
            (seen, last) = (seq, Some(frame));
        }
    }
    panic!("no matching frame in 5 s, last: {last:?}");
}

fn close_to(pixel: [u8; 4], expected: [u8; 4]) -> bool {
    pixel.iter().zip(expected).all(|(&channel, wanted)| channel.abs_diff(wanted) <= 8)
}

/// Окно 400×300 без рамки (всё оно — клиентская область) в своём потоке со
/// своим циклом сообщений, как окно трансляции в чужом процессе.
/// Закрывается при `Drop`.
struct TestWindow {
    /// `HWND` не `Send` — через поток он идёт числом.
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl TestWindow {
    fn open() -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            // SAFETY: обычное создание окна; класс и заголовок — статические
            // строки, сообщения разбирает этот же поток.
            unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                let class = WNDCLASSW {
                    // Выросшее окно перерисовывается целиком, а не только новой полосой.
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance.into(),
                    lpszClassName: w!("AnalyzerCaptureTest"),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let window = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("AnalyzerCaptureTest"),
                    w!("Проверка захвата"),
                    WS_POPUP | WS_VISIBLE,
                    100,
                    100,
                    400,
                    300,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                tx.send(window.0 as isize).unwrap();
                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        });
        Self { hwnd: rx.recv().unwrap(), thread: Some(thread) }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    fn resize(&self, width: i32, height: i32) {
        // SAFETY: окно живо; Windows передаст изменение его потоку и дождётся.
        unsafe {
            SetWindowPos(self.hwnd(), None, 0, 0, width, height, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE)
        }
        .unwrap();
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: окно ещё живо — закрыть его можно только этим сообщением.
        let _ = unsafe { PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_PAINT => {
            // SAFETY: рисование в своём окне между BeginPaint и EndPaint.
            unsafe {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(window, &mut paint);
                let mut client = RECT::default();
                let _ = GetClientRect(window, &mut client);
                let middle = client.right / 2;
                // COLORREF — 0x00BBGGRR.
                for (left, right, color) in [(0, middle, 0x0000FF), (middle, client.right, 0xFF0000)] {
                    let brush = CreateSolidBrush(COLORREF(color));
                    FillRect(dc, &RECT { left, top: 0, right, bottom: client.bottom }, brush);
                    let _ = DeleteObject(brush.into());
                }
                let _ = EndPaint(window, &paint);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: окно закрыто — цикл сообщений его потока кончается.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: всё прочее — обработка по умолчанию.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
