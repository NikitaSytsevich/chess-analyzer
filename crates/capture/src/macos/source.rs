use screencapturekit::content_sharing_picker::{
    SCContentSharingPicker, SCContentSharingPickerConfiguration, SCContentSharingPickerMode, SCPickedSource,
    SCPickerOutcome,
};
use screencapturekit::prelude::SCContentFilter;

use crate::{CaptureError, NativeWindow};

/// Окно, которое комментатор выбрал для захвата.
#[derive(Clone)]
pub struct Source {
    pub(crate) filter: SCContentFilter,
    /// Заголовок окна — например, «YouTube — Google Chrome».
    pub title: String,
    /// Размер содержимого в точках экрана.
    pub size_points: (f64, f64),
    /// Сколько пикселей в одной точке: 2 на Retina.
    pub scale: f64,
    /// Номер окна CoreGraphics — по нему стрелки находят окно на экране.
    pub(crate) window_id: Option<u32>,
}

impl Source {
    pub fn pixel_size(&self) -> (u32, u32) {
        ((self.size_points.0 * self.scale).round() as u32, (self.size_points.1 * self.scale).round() as u32)
    }

    /// Номер выбранного окна (см. [`NativeWindow`]).
    pub fn native_window(&self) -> Option<NativeWindow> {
        self.window_id.map(|id| NativeWindow(u64::from(id)))
    }
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("title", &self.title)
            .field("size_points", &self.size_points)
            .field("scale", &self.scale)
            .field("window_id", &self.window_id)
            .finish_non_exhaustive()
    }
}

/// Показывает системное окно выбора и сообщает выбор в `on_done`.
///
/// `Ok(None)` — комментатор закрыл выбор, ничего не выбрав. Колбэк приходит
/// из очереди ScreenCaptureKit, а не из главного потока: интерфейс должен
/// переложить результат к себе сам.
pub fn pick_source(on_done: impl FnOnce(Result<Option<Source>, CaptureError>) + Send + 'static) {
    if !SCContentSharingPicker::is_available() {
        on_done(Err(CaptureError::PickerUnavailable));
        return;
    }
    let mut config = SCContentSharingPickerConfiguration::new();
    // Трансляция идёт в одном окне браузера или плеера: выбор целого экрана
    // только добавил бы в кадр лишнего — вплоть до окна самого анализатора.
    config.set_allowed_picker_modes(&[SCContentSharingPickerMode::SingleWindow]);
    SCContentSharingPicker::show(&config, move |outcome| {
        let result = match outcome {
            SCPickerOutcome::Picked(picked) => {
                let title = match picked.source() {
                    SCPickedSource::Window(title) => title,
                    SCPickedSource::Application(name) => name,
                    SCPickedSource::Display(id) => format!("Экран {id}"),
                    SCPickedSource::Unknown => "Окно трансляции".to_owned(),
                };
                Ok(Some(Source {
                    filter: picked.filter(),
                    title,
                    size_points: picked.size(),
                    scale: picked.scale().max(1.0),
                    window_id: picked.windows().first().map(|window| window.window_id()),
                }))
            }
            SCPickerOutcome::Cancelled => Ok(None),
            SCPickerOutcome::Error(message) => Err(CaptureError::Picker(message)),
        };
        on_done(result);
    });
}
