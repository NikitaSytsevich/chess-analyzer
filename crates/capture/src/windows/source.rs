use std::sync::{Arc, Mutex};

use windows::Graphics::Capture::{
    GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCapturePicker,
    GraphicsCaptureSession,
};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::UI::Shell::IInitializeWithWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
};
use windows::core::{BOOL, Interface};

use crate::{CaptureError, NativeWindow};

/// Окно, которое комментатор выбрал для захвата.
#[derive(Clone)]
pub struct Source {
    pub(crate) item: GraphicsCaptureItem,
    /// Заголовок окна — например, «YouTube — Google Chrome».
    pub title: String,
    /// Размер окна в пикселях на момент выбора.
    size: (u32, u32),
    /// Само окно — если нашлось (см. `find_window`).
    window: Option<NativeWindow>,
}

impl Source {
    pub fn pixel_size(&self) -> (u32, u32) {
        self.size
    }

    /// HWND выбранного окна (см. [`NativeWindow`]).
    pub fn native_window(&self) -> Option<NativeWindow> {
        self.window
    }
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source").field("title", &self.title).field("size", &self.size).finish_non_exhaustive()
    }
}

/// Показывает системное окно выбора и сообщает выбор в `on_done`.
///
/// Вызывать из потока окна приложения: выбор привязывается к активному окну
/// этого потока. `Ok(None)` — комментатор закрыл выбор, ничего не выбрав.
/// Колбэк приходит из пула потоков Windows, а не из главного потока:
/// интерфейс должен переложить результат к себе сам.
pub fn pick_source(on_done: impl FnOnce(Result<Option<Source>, CaptureError>) + Send + 'static) {
    // Ошибка может случиться и до показа выбора, и после — колбэк один.
    let on_done = Arc::new(Mutex::new(Some(on_done)));
    let report = move |result| {
        if let Some(on_done) = on_done.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            on_done(result);
        }
    };
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        report(Err(CaptureError::PickerUnavailable));
        return;
    }
    ask_borderless();
    let shown = show_picker({
        let report = report.clone();
        move |picked| {
            report(match picked {
                Ok(item) => source(&item).map(Some),
                // Выбор закрыли: вместо окна пришёл null.
                Err(error) if error.code().is_ok() => Ok(None),
                Err(error) => Err(CaptureError::Picker(error.message())),
            });
        }
    });
    if let Err(error) = shown {
        report(Err(CaptureError::Picker(error.message())));
    }
}

/// Убрать жёлтую рамку вокруг захватываемого окна Windows 11 даёт, только
/// если приложение заранее спросило разрешения. Спрашиваем, пока комментатор
/// выбирает окно, — к началу захвата ответ уже есть. На Windows 10 такого
/// разрешения нет, и рамка остаётся.
fn ask_borderless() {
    let asked = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
        .and_then(|request| request.when(|status| tracing::debug!(?status, "borderless capture")));
    if let Err(error) = asked {
        tracing::debug!(%error, "borderless capture is not available");
    }
}

fn show_picker(
    on_picked: impl FnOnce(windows::core::Result<GraphicsCaptureItem>) + Send + 'static,
) -> windows::core::Result<()> {
    let picker = GraphicsCapturePicker::new()?;
    // Выбор из обычного приложения Win32 должен знать окно-владельца.
    // SAFETY: функции только читают, какое окно сейчас активно.
    let mut owner = unsafe { GetActiveWindow() };
    if owner.is_invalid() {
        owner = unsafe { GetForegroundWindow() };
    }
    // SAFETY: `owner` — окно, которое Windows только что вернула; выбор его
    // только запоминает.
    unsafe { picker.cast::<IInitializeWithWindow>()?.Initialize(owner)? };
    picker.PickSingleItemAsync()?.when(on_picked)
}

/// Источник из выбранного окна. Проверки получают окно без системного выбора.
pub(super) fn source(item: &GraphicsCaptureItem) -> Result<Source, CaptureError> {
    let describe = || -> windows::core::Result<(String, (u32, u32))> {
        let size = item.Size()?;
        Ok((item.DisplayName()?.to_string_lossy(), (size.Width.max(1) as u32, size.Height.max(1) as u32)))
    };
    let (title, size) = describe().map_err(|error| CaptureError::Picker(error.message()))?;
    let window = find_window(&title, size).map(|hwnd| NativeWindow(hwnd.0 as usize as u64));
    if window.is_none() {
        tracing::info!(%title, "the picked window is not found among top-level windows");
    }
    let title = if title.trim().is_empty() { "Окно трансляции".to_owned() } else { title };
    Ok(Source { item: item.clone(), title, size, window })
}

/// Окно, которое показывает выбранный источник. Системный выбор его не
/// называет — только заголовок и размер, — а стрелкам поверх трансляции оно
/// нужно. Ищем среди видимых окон с тем же заголовком: сначала того же
/// размера, иначе — любое из них. Выбор только что закрылся, и окно ещё не
/// успело ни переименоваться, ни изменить размер. Из одинаковых берётся
/// верхнее: окна перебираются сверху вниз.
fn find_window(title: &str, size: (u32, u32)) -> Option<HWND> {
    struct Search {
        title: Vec<u16>,
        size: (u32, u32),
        same_size: Option<HWND>,
        same_title: Option<HWND>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` — указатель на `Search`, живой до конца `EnumWindows`.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        // SAFETY: функции только читают сведения об окне, которое прислала
        // система; закрытое за это время окно даст пустой ответ.
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() || window_title(hwnd) != search.title {
            return BOOL::from(true);
        }
        search.same_title.get_or_insert(hwnd);
        if frame_size(hwnd)
            .is_some_and(|found| found.0.abs_diff(search.size.0) <= 2 && found.1.abs_diff(search.size.1) <= 2)
        {
            search.same_size = Some(hwnd);
            return BOOL::from(false);
        }
        BOOL::from(true)
    }

    if title.is_empty() {
        return None;
    }
    let mut search =
        Search { title: title.encode_utf16().collect(), size, same_size: None, same_title: None };
    // SAFETY: `visit` получает указатель на `search`, который живёт дольше
    // перебора. Перебор, остановленный `visit`, возвращает ошибку — это не она.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&raw mut search as isize)) };
    search.same_size.or(search.same_title)
}

/// Заголовок окна в UTF-16.
fn window_title(hwnd: HWND) -> Vec<u16> {
    // SAFETY: только чтение заголовка в буфер нужной длины.
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    let mut buffer = vec![0u16; usize::try_from(length).unwrap_or(0) + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    buffer.truncate(usize::try_from(copied).unwrap_or(0));
    buffer
}

/// Видимые границы окна (без невидимой рамки для изменения размера) — их же
/// снимает захват.
fn frame_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut rect = RECT::default();
    // SAFETY: DWM пишет в `rect` ровно `size_of::<RECT>()` байт.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut rect).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    Some(((rect.right - rect.left).max(0) as u32, (rect.bottom - rect.top).max(0) as u32))
}
