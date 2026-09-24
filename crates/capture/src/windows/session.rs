use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use analyzer_vision::{Frame, FrameSlot};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{E_POINTER, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::core::{IInspectable, Interface};

use super::Source;
use crate::config::fit_pixels;
use crate::pixels::{downscale_bgra, region_pixels};
use crate::{CaptureConfig, CaptureError, CaptureStats, RegionF};

/// Кадры в BGRA — в том же формате, что ждёт распознавание.
const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
/// Буферов в пуле кадров: один ждёт потока выдачи, в другой Windows пишет
/// следующий кадр.
const BUFFERS: i32 = 2;

/// Идущий захват выбранного окна. Остановка — `Drop`.
///
/// Windows присылает кадр, только когда окно изменилось, зато сколько угодно
/// часто. Поток выдачи копирует кадр на видеокарте в «последний кадр» и
/// отдаёт его получателю не чаще `fps` раз в секунду. Кадр, пришедший раньше
/// срока, не теряется, а ждёт срока: иначе после хода на неподвижной доске
/// распознавание осталось бы с позицией до хода — следующего кадра Windows
/// не пришлёт, пока доска не изменится.
///
/// С видеокартой работает только поток выдачи; обработчик кадров Windows
/// лишь передаёт ему кадр. Windows вызывает обработчик, держа блокировку
/// устройства Direct3D, и обработчик, который ждал бы поток выдачи посреди
/// его вызова Direct3D, намертво сцепился бы с ним.
pub struct CaptureSession {
    source: Source,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    frame_arrived: i64,
    closed: i64,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    /// Будит поток выдачи: пришёл кадр, сменились настройки или захват остановлен.
    wake: Condvar,
    /// Куда кладутся кадры. Меняется на ходу: пока комментатор выбирает
    /// доску, кадры окна идут экрану настройки, потом — распознаванию.
    target: Mutex<Arc<FrameSlot>>,
    frames: AtomicU64,
}

struct State {
    config: CaptureConfig,
    /// Кадр от Windows, который поток выдачи ещё не забрал, и когда он пришёл.
    arrived: Option<(Direct3D11CaptureFrame, Instant)>,
    /// Последний кадр нужно отдать заново: сменились настройки или получатель.
    resend: bool,
    stop: bool,
}

impl CaptureSession {
    /// Запускает захват. Кадры кладутся в `slot`, закрытие окна трансляции
    /// сообщается в `on_stop`.
    pub fn start(
        source: Source,
        config: CaptureConfig,
        slot: Arc<FrameSlot>,
        on_stop: impl Fn(Option<String>) + Send + Sync + 'static,
    ) -> Result<Self, CaptureError> {
        let gpu = Gpu::new().map_err(stream_error)?;
        let size = source.item.Size().map_err(stream_error)?;
        let pool = winrt_device(&gpu.device)
            .and_then(|device| Direct3D11CaptureFramePool::CreateFreeThreaded(&device, FORMAT, BUFFERS, size))
            .map_err(stream_error)?;
        let session = pool.CreateCaptureSession(&source.item).map_err(stream_error)?;
        // Курсор над доской распознавание приняло бы за часть фигуры. Отключается
        // с Windows 10 2004; раньше курсора в кадре просто не избежать.
        let _ = session.SetIsCursorCaptureEnabled(false);
        // Без жёлтой рамки вокруг окна — если Windows 11 разрешила (о
        // разрешении спросил `pick_source`). На Windows 10 вызов не пройдёт,
        // без разрешения — пройдёт впустую; рамка тогда останется.
        let _ = session.SetIsBorderRequired(false);

        let shared = Arc::new(Shared {
            state: Mutex::new(State { config, arrived: None, resend: false, stop: false }),
            wake: Condvar::new(),
            target: Mutex::new(slot),
            frames: AtomicU64::new(0),
        });
        let arrived = Arc::clone(&shared);
        let device = gpu.device.clone();
        let pool_size = Mutex::new(size);
        let frame_arrived = pool
            .FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                move |pool, _| {
                    if let Err(error) = arrived.arrive(pool.ok()?, &device, &pool_size) {
                        tracing::debug!(%error, "capture frame dropped");
                    }
                    Ok(())
                },
            ))
            .map_err(stream_error)?;
        let closed = match source.item.Closed(&TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(
            move |_, _| {
                on_stop(None);
                Ok(())
            },
        )) {
            Ok(token) => token,
            Err(error) => {
                let _ = pool.RemoveFrameArrived(frame_arrived);
                return Err(stream_error(error));
            }
        };

        let worker = Arc::clone(&shared);
        let mut capture = Self { source, pool, session, frame_arrived, closed, shared, worker: None };
        capture.worker = Some(
            std::thread::Builder::new()
                .name("capture".into())
                .spawn(move || worker.run(gpu))
                .map_err(|error| CaptureError::Stream(error.to_string()))?,
        );
        // Ошибка здесь роняет `capture`, и `Drop` отпишется и остановит поток.
        capture.session.StartCapture().map_err(stream_error)?;
        tracing::info!(title = %capture.source.title, ?config, "capture started");
        Ok(capture)
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Переключает захват на область окна (или на окно целиком).
    pub fn set_region(&self, region: Option<RegionF>) -> Result<(), CaptureError> {
        let config = CaptureConfig { region, ..self.config() };
        self.reconfigure(config)
    }

    /// Новые частота, область и размер кадра. Захват не перезапускается:
    /// Windows всё равно присылает окно целиком, а область вырезается здесь.
    pub fn reconfigure(&self, config: CaptureConfig) -> Result<(), CaptureError> {
        let mut state = lock(&self.shared.state);
        state.config = config;
        // Последний кадр уходит заново, уже с новой областью: на неподвижной
        // доске нового кадра от Windows можно ждать долго.
        state.resend = true;
        self.shared.wake.notify_all();
        Ok(())
    }

    pub fn config(&self) -> CaptureConfig {
        lock(&self.shared.state).config
    }

    /// Следующие кадры пойдут в `slot` — начиная с последнего кадра окна,
    /// чтобы новому получателю не ждать изменений в окне.
    pub fn set_target(&self, slot: Arc<FrameSlot>) {
        *lock(&self.shared.target) = slot;
        lock(&self.shared.state).resend = true;
        self.shared.wake.notify_all();
    }

    pub fn stats(&self) -> CaptureStats {
        // Кадров «без изменений» Windows не присылает вовсе: `idle` всегда 0.
        CaptureStats { frames: self.shared.frames.load(Ordering::Relaxed), idle: 0 }
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.frame_arrived);
        let _ = self.source.item.RemoveClosed(self.closed);
        if let Err(error) = self.session.Close().and_then(|()| self.pool.Close()) {
            tracing::warn!(%error, "capture did not stop cleanly");
        }
        lock(&self.shared.state).stop = true;
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Shared {
    /// Кадр от Windows (из её пула потоков) — потоку выдачи. Пул растёт
    /// вместе с окном.
    fn arrive(
        &self,
        pool: &Direct3D11CaptureFramePool,
        device: &ID3D11Device,
        pool_size: &Mutex<SizeInt32>,
    ) -> windows::core::Result<()> {
        let frame = pool.TryGetNextFrame()?;
        let size = frame.ContentSize()?;
        // Свёрнутое окно присылает пустой кадр — в нём нечего смотреть.
        if size.Width <= 0 || size.Height <= 0 {
            return frame.Close();
        }
        // Буферы пула не меньше окна: содержимое лежит в их левом верхнем
        // углу. Окно выросло — кадр обрезан по старому буферу: пул растёт, а
        // кадр пропускается, следующий придёт в новом размере.
        let mut pool_size = lock(pool_size);
        if size.Width > pool_size.Width || size.Height > pool_size.Height {
            frame.Close()?;
            // Пересоздание выбрасывает выданные кадры — и тот, что ждёт выдачи.
            lock(&self.state).arrived = None;
            pool.Recreate(&winrt_device(device)?, FORMAT, BUFFERS, size)?;
            *pool_size = size;
            return Ok(());
        }
        // Кадр, который поток выдачи не успел забрать, вытесняется новым и
        // возвращается в пул.
        lock(&self.state).arrived = Some((frame, Instant::now()));
        self.wake.notify_all();
        Ok(())
    }

    /// Поток выдачи: свежий кадр — получателю, но не чаще `fps` раз в секунду.
    fn run(&self, mut gpu: Gpu) {
        let mut last_sent: Option<Instant> = None;
        let mut failing = false;
        loop {
            let mut state = self
                .wake
                .wait_while(lock(&self.state), |state| {
                    state.arrived.is_none() && !state.resend && !state.stop
                })
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.stop {
                return;
            }
            let interval = Duration::from_secs(1) / state.config.fps.max(1);
            if let Some(due) = last_sent.map(|sent| sent + interval) {
                let now = Instant::now();
                if now < due {
                    // Срок не наступил: ждём его, а остановка разбудит раньше.
                    drop(self.wake.wait_timeout_while(state, due - now, |state| !state.stop));
                    continue;
                }
            }
            let arrived = state.arrived.take();
            state.resend = false;
            let config = state.config;
            drop(state);

            last_sent = Some(Instant::now());
            let stored = match arrived {
                // Кадр возвращается в пул сразу после копирования.
                Some((frame, captured_at)) => gpu.store(&frame, captured_at).and_then(|()| frame.Close()),
                None => Ok(()),
            };
            match stored.and_then(|()| gpu.read(&config)) {
                Ok(Some(frame)) => {
                    failing = false;
                    self.frames.fetch_add(1, Ordering::Relaxed);
                    let slot = Arc::clone(&lock(&self.target));
                    slot.put(frame);
                }
                Ok(None) => {}
                Err(error) => {
                    // Сбой повторится на каждом кадре — пишем о нём один раз.
                    if !failing {
                        tracing::warn!(%error, "capture frame could not be read");
                    }
                    failing = true;
                }
            }
        }
    }
}

/// Видеокарта: последний кадр окна и его копия для чтения процессором.
/// Живёт в потоке выдачи.
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// Последний кадр окна целиком, в размере содержимого окна.
    latest: Option<Texture>,
    /// Когда пришёл последний кадр.
    captured_at: Instant,
    /// Область доски из последнего кадра, доступная процессору.
    staging: Option<Texture>,
}

struct Texture {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

impl Gpu {
    fn new() -> windows::core::Result<Self> {
        // Без видеокарты с Direct3D 11 (в виртуальной машине, по удалённому
        // рабочему столу) — программная WARP: медленнее, но работает.
        let (device, context) =
            create_device(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| create_device(D3D_DRIVER_TYPE_WARP))?;
        // Устройством из своих потоков пользуется и сама Windows: пусть
        // Direct3D упорядочивает вызовы сам.
        if let Ok(multithread) = context.cast::<ID3D11Multithread>() {
            // SAFETY: только включает внутреннюю блокировку устройства.
            unsafe {
                let _ = multithread.SetMultithreadProtected(true);
            }
        }
        Ok(Self { device, context, latest: None, captured_at: Instant::now(), staging: None })
    }

    /// Копирует содержимое кадра (оно в левом верхнем углу буфера) в
    /// последний кадр — на видеокарте, без чтения процессором.
    fn store(&mut self, frame: &Direct3D11CaptureFrame, captured_at: Instant) -> windows::core::Result<()> {
        let size = frame.ContentSize()?;
        let (width, height) = (size.Width.max(1) as u32, size.Height.max(1) as u32);
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        // SAFETY: буфер кадра живёт, пока кадр не закрыт, а закрывается он
        // после копирования.
        let surface: ID3D11Texture2D = unsafe { access.GetInterface() }?;
        let latest = texture(&self.device, &mut self.latest, width, height, false)?;
        let area = D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
        // SAFETY: обе текстуры одного формата, `area` лежит внутри источника
        // и совпадает по размеру с `latest`.
        unsafe { self.context.CopySubresourceRegion(&latest, 0, 0, 0, 0, &surface, 0, Some(&area)) };
        self.captured_at = captured_at;
        Ok(())
    }

    /// Область из последнего кадра, уменьшенная по длинной стороне, — или
    /// `None`, если кадров ещё не было.
    fn read(&mut self, config: &CaptureConfig) -> windows::core::Result<Option<Frame>> {
        let Some(latest) = &self.latest else { return Ok(None) };
        let area = region_pixels(config.region.unwrap_or(RegionF::FULL), latest.width, latest.height);
        let latest = latest.texture.clone();
        let staging = texture(&self.device, &mut self.staging, area.width, area.height, true)?;
        let src = D3D11_BOX {
            left: area.x,
            top: area.y,
            front: 0,
            right: area.x + area.width,
            bottom: area.y + area.height,
            back: 1,
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: `src` лежит внутри `latest` и совпадает по размеру со
        // `staging`; `staging` создана для чтения процессором.
        unsafe {
            self.context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &latest, 0, Some(&src));
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }
        let stride = mapped.RowPitch as usize;
        let row = area.width as usize * 4;
        // SAFETY: пока текстура отображена, в `pData` лежат `height` строк по
        // `RowPitch` байт (у последней — хотя бы `row`). Срез копируется в
        // кадр до `Unmap`.
        let bytes = unsafe {
            std::slice::from_raw_parts(mapped.pData as *const u8, stride * (area.height as usize - 1) + row)
        };
        let max_side = if config.region.is_some() { config.max_side_region } else { config.max_side_full };
        let (width, height) = fit_pixels(f64::from(area.width), f64::from(area.height), max_side);
        let frame = if (width, height) == (area.width, area.height) {
            Frame::from_strided(width, height, stride, bytes, self.captured_at)
        } else {
            let pixels = downscale_bgra(bytes, area.width, area.height, stride, width, height);
            Frame::new(width, height, pixels, self.captured_at)
        };
        // SAFETY: `bytes` больше не используется.
        unsafe { self.context.Unmap(&staging, 0) };
        Ok(Some(frame))
    }
}

fn create_device(driver: D3D_DRIVER_TYPE) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut device, mut context) = (None, None);
    // SAFETY: указатели на результат живут до конца вызова.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    device.zip(context).ok_or_else(|| E_POINTER.into())
}

/// То же устройство — в виде, который принимает Windows.Graphics.Capture.
fn winrt_device(device: &ID3D11Device) -> windows::core::Result<IDirect3DDevice> {
    let dxgi: IDXGIDevice = device.cast()?;
    // SAFETY: `dxgi` — живое устройство, функция только оборачивает его.
    unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?.cast()
}

/// Текстура BGRA размером `width × height`: прежняя из `slot`, если размер
/// тот же, иначе новая. `for_cpu` — копия для чтения процессором, иначе —
/// для копий на видеокарте.
fn texture(
    device: &ID3D11Device,
    slot: &mut Option<Texture>,
    width: u32,
    height: u32,
    for_cpu: bool,
) -> windows::core::Result<ID3D11Texture2D> {
    if let Some(existing) = slot
        && (existing.width, existing.height) == (width, height)
    {
        return Ok(existing.texture.clone());
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: if for_cpu { D3D11_USAGE_STAGING } else { D3D11_USAGE_DEFAULT },
        BindFlags: 0,
        CPUAccessFlags: if for_cpu { D3D11_CPU_ACCESS_READ.0 as u32 } else { 0 },
        MiscFlags: 0,
    };
    let mut created = None;
    // SAFETY: `desc` и указатель на результат живут до конца вызова.
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut created)) }?;
    let created: ID3D11Texture2D = created.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
    *slot = Some(Texture { texture: created.clone(), width, height });
    Ok(created)
}

fn stream_error(error: windows::core::Error) -> CaptureError {
    CaptureError::Stream(error.message())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
