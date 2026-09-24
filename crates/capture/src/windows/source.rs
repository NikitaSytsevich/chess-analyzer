use std::sync::{Arc, Mutex};

use windows::Graphics::Capture::{
    GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCapturePicker,
    GraphicsCaptureSession,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::UI::Shell::IInitializeWithWindow;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::core::Interface;

use crate::CaptureError;

/// Окно, которое комментатор выбрал для захвата.
#[derive(Clone)]
pub struct Source {
    pub(crate) item: GraphicsCaptureItem,
    /// Заголовок окна — например, «YouTube — Google Chrome».
    pub title: String,
    /// Размер окна в пикселях на момент выбора.
    size: (u32, u32),
}

impl Source {
    pub fn pixel_size(&self) -> (u32, u32) {
        self.size
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
    let title = if title.trim().is_empty() { "Окно трансляции".to_owned() } else { title };
    Ok(Source { item: item.clone(), title, size })
}
