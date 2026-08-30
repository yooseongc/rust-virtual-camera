use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{imageops::FilterType, DynamicImage, GenericImageView};
use libloading::Library;
use memmap2::MmapMut;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::{
    ffi::OsStr,
    os::windows::{ffi::OsStrExt, process::CommandExt},
};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{fence, AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime, State, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
    },
    System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE},
    UI::{
        Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
        WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN, SW_SHOWNORMAL,
        },
    },
};

const MEDIA_SOURCE_CLSID: &str = "{CD31FFCF-F7BE-42DC-A072-F49AD0E66AF7}";
const DIRECTSHOW_CLSID: &str = "{AEF3B972-5FA5-4647-9571-358EB472BC9E}";
const SHARED_FRAME_MAGIC: u32 = 0x4D43_5652;
const SHARED_FRAME_VERSION: u32 = 1;
const SHARED_FRAME_HEADER_SIZE: usize = 64;
const SHARED_FRAME_MAX_WIDTH: usize = 3840;
const SHARED_FRAME_MAX_HEIGHT: usize = 2160;
const SHARED_FRAME_FILE_SIZE: usize =
    SHARED_FRAME_HEADER_SIZE + SHARED_FRAME_MAX_WIDTH * SHARED_FRAME_MAX_HEIGHT * 4;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CameraConfig {
    #[serde(default = "default_backend")]
    backend: String,
    width: u32,
    height: u32,
    fps: u32,
    source: String,
    image_path: Option<String>,
    color: String,
    mirror: bool,
    #[serde(default)]
    capture_region: Option<CaptureRegion>,
}

fn default_backend() -> String {
    "media-foundation".into()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureRegion {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            width: 1280,
            height: 720,
            fps: 30,
            source: "test-pattern".into(),
            image_path: None,
            color: "#7357ff".into(),
            mirror: false,
            capture_region: None,
        }
    }
}

impl CameraConfig {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.backend.as_str(), "media-foundation" | "directshow") {
            return Err("지원하지 않는 카메라 출력 방식입니다.".into());
        }
        if self.width < 320 || self.height < 240 || self.width > 3840 || self.height > 2160 {
            return Err("해상도는 320×240에서 3840×2160 사이여야 합니다.".into());
        }
        if self.width % 4 != 0 || self.height % 4 != 0 {
            return Err("가로와 세로 크기는 4의 배수여야 합니다.".into());
        }
        if !(1..=60).contains(&self.fps) {
            return Err("프레임률은 1에서 60 FPS 사이여야 합니다.".into());
        }
        if !matches!(
            self.source.as_str(),
            "test-pattern" | "solid" | "image" | "screen-region"
        ) {
            return Err("지원하지 않는 영상 소스입니다.".into());
        }
        if self.source == "image" && self.image_path.as_deref().unwrap_or_default().is_empty() {
            return Err("먼저 이미지 파일을 선택해 주세요.".into());
        }
        if self.source == "screen-region" {
            let region = self
                .capture_region
                .ok_or_else(|| "먼저 송출할 화면 영역을 선택해 주세요.".to_string())?;
            if region.width < 16 || region.height < 16 {
                return Err("화면 영역은 가로와 세로가 각각 16픽셀 이상이어야 합니다.".into());
            }
        }
        parse_hex_color(&self.color)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    camera: CameraConfig,
    start_stream_on_launch: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            camera: CameraConfig::default(),
            start_stream_on_launch: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    streaming: bool,
    connected: bool,
    message: String,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DriverStatus {
    media_foundation_installed: bool,
    direct_show_installed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitialState {
    settings: Settings,
    runtime: RuntimeStatus,
    driver: DriverStatus,
    autostart: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectedImage {
    path: String,
    name: String,
    preview_data_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopPreview {
    preview_data_url: String,
    virtual_x: i32,
    virtual_y: i32,
    virtual_width: u32,
    virtual_height: u32,
    preview_width: u32,
    preview_height: u32,
}

struct StreamHandle {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl StreamHandle {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Default)]
struct AppState {
    settings: Mutex<Settings>,
    runtime: Arc<Mutex<RuntimeStatus>>,
    stream: Mutex<Option<StreamHandle>>,
}

fn settings_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("설정 폴더를 확인할 수 없습니다: {error}"))?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("설정 폴더를 만들 수 없습니다: {error}"))?;
    Ok(directory.join("settings.json"))
}

fn load_settings<R: Runtime>(app: &AppHandle<R>) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

fn save_settings<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> Result<(), String> {
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("설정을 직렬화할 수 없습니다: {error}"))?;
    fs::write(settings_path(app)?, contents)
        .map_err(|error| format!("설정을 저장할 수 없습니다: {error}"))
}

fn development_native_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("native")
        .join(relative)
}

fn packaged_native_path<R: Runtime>(app: &AppHandle<R>, relative: &str) -> Option<PathBuf> {
    app.path()
        .resource_dir()
        .ok()
        .map(|directory| directory.join(relative))
        .filter(|path| path.exists())
}

fn native_file<R: Runtime>(app: &AppHandle<R>, file: &str) -> Result<PathBuf, String> {
    let (packaged, development) = match file {
        "mfvcam_manager.exe" => (
            "mfvcam/mfvcam_manager.exe",
            "mfvcam-manager/dist/mfvcam_manager.exe",
        ),
        "RustVirtualCameraMediaSource.dll" => (
            "mfvcam/RustVirtualCameraMediaSource.dll",
            "windows-camera-reference/Samples/VirtualCamera/x64/Release/VirtualCameraMediaSource.dll",
        ),
        "dshow_manager64.exe" => (
            "dshow/dshow_manager64.exe",
            "dshow-manager/dist/x64/dshow_manager.exe",
        ),
        "dshow_manager32.exe" => (
            "dshow/dshow_manager32.exe",
            "dshow-manager/dist/Win32/dshow_manager.exe",
        ),
        "softcam64.dll" => (
            "dshow/softcam64.dll",
            "softcam/x64/Release/softcam.dll",
        ),
        "softcam32.dll" => (
            "dshow/softcam32.dll",
            "softcam/Win32/Release/softcam.dll",
        ),
        _ => return Err("알 수 없는 네이티브 구성요소입니다.".into()),
    };
    if let Some(path) = packaged_native_path(app, packaged) {
        return Ok(path);
    }
    let path = development_native_path(development);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("필수 구성요소가 없습니다: {}", path.display()))
    }
}

fn registry_clsid_points_to(clsid: &str, view: &str, expected_file: &str) -> bool {
    let output = Command::new("reg.exe")
        .args([
            "query",
            &format!("HKLM\\SOFTWARE\\Classes\\CLSID\\{clsid}\\InprocServer32"),
            view,
        ])
        .output();
    output
        .map(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .to_ascii_lowercase()
                    .contains(&expected_file.to_ascii_lowercase())
        })
        .unwrap_or(false)
}

fn driver_status() -> DriverStatus {
    DriverStatus {
        media_foundation_installed: registry_clsid_points_to(
            MEDIA_SOURCE_CLSID,
            "/reg:64",
            "RustVirtualCameraMediaSource-0.3.0.dll",
        ),
        direct_show_installed: registry_clsid_points_to(
            DIRECTSHOW_CLSID,
            "/reg:64",
            "RustVirtualCameraDirectShow64-0.3.0.dll",
        ) && registry_clsid_points_to(
            DIRECTSHOW_CLSID,
            "/reg:32",
            "RustVirtualCameraDirectShow32-0.3.0.dll",
        ),
    }
}

fn shared_frame_path() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("RustVirtualCamera")
        .join("frame.bin")
}

#[cfg(windows)]
fn ensure_virtual_camera<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let manager = native_file(app, "mfvcam_manager.exe")?;
    let status = Command::new(manager)
        .arg("ensure")
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|error| format!("Windows 가상 카메라를 활성화할 수 없습니다: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Windows 가상 카메라 활성화 도구가 오류 코드 {}로 종료됐습니다.",
            status.code().unwrap_or(-1)
        ))
    }
}

#[cfg(windows)]
fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn run_elevated(executable: &Path, parameters: &str) -> Result<(), String> {
    let verb = wide_null(OsStr::new("runas"));
    let file = wide_null(executable.as_os_str());
    let parameters = wide_null(OsStr::new(parameters));
    let mut execute_info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };

    if unsafe { ShellExecuteExW(&mut execute_info) } == 0 {
        return Err(format!(
            "관리자 권한으로 실행할 수 없습니다: {}",
            std::io::Error::last_os_error()
        ));
    }
    if execute_info.hProcess.is_null() {
        return Err("권한 상승 프로세스 핸들을 가져올 수 없습니다.".into());
    }

    let wait_result = unsafe { WaitForSingleObject(execute_info.hProcess, INFINITE) };
    if wait_result != WAIT_OBJECT_0 {
        let _ = unsafe { CloseHandle(execute_info.hProcess) };
        return Err(format!("드라이버 관리 도구 대기 오류: {wait_result}"));
    }

    let mut exit_code = 0_u32;
    let exit_code_result = unsafe { GetExitCodeProcess(execute_info.hProcess, &mut exit_code) };
    let _ = unsafe { CloseHandle(execute_info.hProcess) };
    if exit_code_result == 0 {
        return Err(format!(
            "드라이버 관리 결과를 확인할 수 없습니다: {}",
            std::io::Error::last_os_error()
        ));
    }
    if exit_code != 0 {
        return Err(format!(
            "드라이버 관리 도구가 오류 코드 {exit_code}로 종료됐습니다."
        ));
    }
    Ok(())
}

fn parse_hex_color(value: &str) -> Result<[u8; 3], String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return Err("색상은 #RRGGBB 형식이어야 합니다.".into());
    }
    let red = u8::from_str_radix(&hex[0..2], 16).map_err(|_| "잘못된 색상입니다.")?;
    let green = u8::from_str_radix(&hex[2..4], 16).map_err(|_| "잘못된 색상입니다.")?;
    let blue = u8::from_str_radix(&hex[4..6], 16).map_err(|_| "잘못된 색상입니다.")?;
    Ok([red, green, blue])
}

#[cfg(windows)]
fn virtual_screen_region() -> Result<CaptureRegion, String> {
    let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err("Windows 데스크톱 크기를 확인할 수 없습니다.".into());
    }
    Ok(CaptureRegion {
        x,
        y,
        width: width as u32,
        height: height as u32,
    })
}

#[cfg(windows)]
fn capture_screen_bgra(
    region: CaptureRegion,
    output_width: u32,
    output_height: u32,
) -> Result<Vec<u8>, String> {
    if region.width == 0 || region.height == 0 || output_width == 0 || output_height == 0 {
        return Err("캡처 영역 또는 출력 크기가 올바르지 않습니다.".into());
    }
    let capture_length = region.width as usize * region.height as usize * 4;
    let mut pixels = vec![0_u8; capture_length];

    unsafe {
        let screen_dc = GetDC(std::ptr::null_mut());
        if screen_dc.is_null() {
            return Err("화면 캡처 장치를 열 수 없습니다.".into());
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            return Err("화면 캡처 버퍼를 만들 수 없습니다.".into());
        }
        let mut bitmap_info: BITMAPINFO = std::mem::zeroed();
        bitmap_info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: region.width as i32,
            biHeight: -(region.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bitmap_bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            screen_dc,
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bitmap_bits,
            std::ptr::null_mut(),
            0,
        );
        if bitmap.is_null() {
            DeleteDC(memory_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            return Err("화면 캡처 비트맵을 만들 수 없습니다.".into());
        }

        let previous = SelectObject(memory_dc, bitmap);
        let copied = BitBlt(
            memory_dc,
            0,
            0,
            region.width as i32,
            region.height as i32,
            screen_dc,
            region.x,
            region.y,
            SRCCOPY | CAPTUREBLT,
        );
        if copied != 0 && !bitmap_bits.is_null() {
            std::ptr::copy_nonoverlapping(
                bitmap_bits.cast::<u8>(),
                pixels.as_mut_ptr(),
                capture_length,
            );
        }
        SelectObject(memory_dc, previous);

        DeleteObject(bitmap);
        DeleteDC(memory_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);

        if copied == 0 || bitmap_bits.is_null() {
            return Err("선택한 화면 영역을 캡처할 수 없습니다.".into());
        }
    }

    if region.width == output_width && region.height == output_height {
        return Ok(pixels);
    }
    let image = image::RgbaImage::from_raw(region.width, region.height, pixels)
        .ok_or_else(|| "캡처 프레임 크기가 올바르지 않습니다.".to_string())?;
    Ok(
        image::imageops::resize(&image, output_width, output_height, FilterType::Triangle)
            .into_raw(),
    )
}

fn mirror_bgra(frame: &mut [u8], width: u32, height: u32) {
    let row_length = width as usize * 4;
    for y in 0..height as usize {
        let row = &mut frame[y * row_length..(y + 1) * row_length];
        for x in 0..width as usize / 2 {
            let opposite = width as usize - 1 - x;
            for channel in 0..4 {
                row.swap(x * 4 + channel, opposite * 4 + channel);
            }
        }
    }
}

#[cfg(windows)]
fn screen_region_frame(config: &CameraConfig) -> Result<Vec<u8>, String> {
    let region = config
        .capture_region
        .ok_or_else(|| "먼저 송출할 화면 영역을 선택해 주세요.".to_string())?;
    let mut frame = capture_screen_bgra(region, config.width, config.height)?;
    if config.mirror {
        mirror_bgra(&mut frame, config.width, config.height);
    }
    Ok(frame)
}

fn fit_image(image: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    let (source_width, source_height) = image.dimensions();
    let scale = (width as f64 / source_width as f64).max(height as f64 / source_height as f64);
    let resized_width = (source_width as f64 * scale).ceil() as u32;
    let resized_height = (source_height as f64 * scale).ceil() as u32;
    let resized = image.resize_exact(resized_width, resized_height, FilterType::Triangle);
    resized.crop_imm(
        (resized_width - width) / 2,
        (resized_height - height) / 2,
        width,
        height,
    )
}

fn load_image_frame(config: &CameraConfig) -> Result<Vec<u8>, String> {
    let path = config
        .image_path
        .as_deref()
        .ok_or_else(|| "이미지 파일이 선택되지 않았습니다.".to_string())?;
    let image = image::open(path).map_err(|error| format!("이미지를 열 수 없습니다: {error}"))?;
    let rgb = fit_image(&image, config.width, config.height).to_rgb8();
    let mut frame = vec![0_u8; (config.width * config.height * 4) as usize];
    for y in 0..config.height {
        for x in 0..config.width {
            let source_x = if config.mirror {
                config.width - 1 - x
            } else {
                x
            };
            let pixel = rgb.get_pixel(source_x, y).0;
            let offset = ((y * config.width + x) * 4) as usize;
            frame[offset..offset + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
    }
    Ok(frame)
}

fn solid_frame(config: &CameraConfig) -> Result<Vec<u8>, String> {
    let [red, green, blue] = parse_hex_color(&config.color)?;
    let mut frame = vec![0_u8; (config.width * config.height * 4) as usize];
    for pixel in frame.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[blue, green, red, 255]);
    }
    Ok(frame)
}

fn test_pattern_frame(config: &CameraConfig, frame_index: u64, frame: &mut [u8]) {
    const BARS: [[u8; 3]; 8] = [
        [235, 235, 235],
        [235, 235, 16],
        [16, 235, 235],
        [16, 235, 16],
        [235, 16, 235],
        [235, 16, 16],
        [16, 16, 235],
        [20, 20, 24],
    ];
    let scan_x = ((frame_index * 8) % config.width as u64) as u32;
    for y in 0..config.height {
        for x in 0..config.width {
            let logical_x = if config.mirror {
                config.width - 1 - x
            } else {
                x
            };
            let bar = ((logical_x as usize * BARS.len()) / config.width as usize).min(7);
            let mut rgb = BARS[bar];
            if y > config.height * 3 / 4 {
                rgb = if ((logical_x / 48) + (y / 48)) % 2 == 0 {
                    [34, 36, 44]
                } else {
                    [15, 16, 20]
                };
            }
            if logical_x.abs_diff(scan_x) < 5 {
                rgb = [255, 255, 255];
            }
            let offset = ((y * config.width + x) * 4) as usize;
            frame[offset..offset + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
    }
}

struct SharedFrameWriter {
    map: MmapMut,
}

impl SharedFrameWriter {
    fn open() -> Result<Self, String> {
        let path = shared_frame_path();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "공유 프레임 파일을 열 수 없습니다. 카메라 구성요소를 다시 설치해 주세요 ({}): {error}",
                    path.display()
                )
            })?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("공유 프레임 파일 크기를 확인할 수 없습니다: {error}"))?;
        if metadata.len() < SHARED_FRAME_FILE_SIZE as u64 {
            return Err("공유 프레임 파일 크기가 올바르지 않습니다. 다시 설치해 주세요.".into());
        }
        let mut map = unsafe { MmapMut::map_mut(&file) }
            .map_err(|error| format!("공유 프레임 메모리를 열 수 없습니다: {error}"))?;
        write_u32(&mut map, 0, SHARED_FRAME_MAGIC);
        write_u32(&mut map, 4, SHARED_FRAME_VERSION);
        Ok(Self { map })
    }

    fn write_frame(&mut self, config: &CameraConfig, frame: &[u8]) -> Result<(), String> {
        let expected_length = config.width as usize * config.height as usize * 4;
        if frame.len() != expected_length
            || expected_length > SHARED_FRAME_FILE_SIZE - SHARED_FRAME_HEADER_SIZE
        {
            return Err("생성된 프레임 크기가 올바르지 않습니다.".into());
        }

        let sequence = unsafe { &*(self.map.as_ptr().add(8).cast::<AtomicU64>()) };
        let current = sequence.load(Ordering::Acquire);
        let writing = if current & 1 == 0 {
            current.wrapping_add(1)
        } else {
            current.wrapping_add(2)
        };
        sequence.store(writing, Ordering::Release);
        fence(Ordering::Release);

        write_u32(&mut self.map, 16, config.width);
        write_u32(&mut self.map, 20, config.height);
        write_u32(&mut self.map, 24, config.width * 4);
        write_u32(&mut self.map, 28, expected_length as u32);
        write_u32(&mut self.map, 32, config.fps);
        write_u32(&mut self.map, 36, 1);
        self.map[SHARED_FRAME_HEADER_SIZE..SHARED_FRAME_HEADER_SIZE + expected_length]
            .copy_from_slice(frame);

        fence(Ordering::Release);
        sequence.store(writing.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    fn heartbeat(&self) -> u64 {
        u64::from_le_bytes(self.map[40..48].try_into().expect("heartbeat bytes"))
    }

    fn set_inactive(&mut self) {
        write_u32(&mut self.map, 36, 0);
        let sequence = unsafe { &*(self.map.as_ptr().add(8).cast::<AtomicU64>()) };
        let current = sequence.load(Ordering::Acquire);
        sequence.store((current | 1).wrapping_add(1), Ordering::Release);
        let _ = self.map.flush_async();
    }
}

impl Drop for SharedFrameWriter {
    fn drop(&mut self) {
        self.set_inactive();
    }
}

type DirectShowHandle = *mut std::ffi::c_void;
type DirectShowCreate = unsafe extern "C" fn(i32, i32, f32) -> DirectShowHandle;
type DirectShowDelete = unsafe extern "C" fn(DirectShowHandle);
type DirectShowSend = unsafe extern "C" fn(DirectShowHandle, *const std::ffi::c_void);
type DirectShowConnected = unsafe extern "C" fn(DirectShowHandle) -> bool;

struct DirectShowCamera {
    _library: Library,
    handle: DirectShowHandle,
    delete: DirectShowDelete,
    send: DirectShowSend,
    connected: DirectShowConnected,
}

impl DirectShowCamera {
    fn open(path: &Path, config: &CameraConfig) -> Result<Self, String> {
        unsafe {
            let library = Library::new(path)
                .map_err(|error| format!("DirectShow 송출 라이브러리를 열 수 없습니다: {error}"))?;
            let create: DirectShowCreate = *library
                .get(b"scCreateCamera\0")
                .map_err(|error| format!("DirectShow 생성 함수를 찾을 수 없습니다: {error}"))?;
            let delete: DirectShowDelete = *library
                .get(b"scDeleteCamera\0")
                .map_err(|error| format!("DirectShow 종료 함수를 찾을 수 없습니다: {error}"))?;
            let send: DirectShowSend = *library
                .get(b"scSendFrame\0")
                .map_err(|error| format!("DirectShow 전송 함수를 찾을 수 없습니다: {error}"))?;
            let connected: DirectShowConnected = *library
                .get(b"scIsConnected\0")
                .map_err(|error| format!("DirectShow 상태 함수를 찾을 수 없습니다: {error}"))?;
            let handle = create(config.width as i32, config.height as i32, config.fps as f32);
            if handle.is_null() {
                return Err(
                    "DirectShow 카메라를 시작할 수 없습니다. 다른 송출 인스턴스를 종료해 주세요."
                        .into(),
                );
            }
            Ok(Self {
                _library: library,
                handle,
                delete,
                send,
                connected,
            })
        }
    }

    fn send_bgra(&self, frame: &[u8], bgr: &mut Vec<u8>) {
        bgr.clear();
        bgr.reserve(frame.len() / 4 * 3);
        for pixel in frame.chunks_exact(4) {
            bgr.extend_from_slice(&pixel[..3]);
        }
        unsafe { (self.send)(self.handle, bgr.as_ptr().cast()) };
    }

    fn is_connected(&self) -> bool {
        unsafe { (self.connected)(self.handle) }
    }
}

impl Drop for DirectShowCamera {
    fn drop(&mut self) {
        unsafe { (self.delete)(self.handle) };
    }
}

fn write_u32(target: &mut [u8], offset: usize, value: u32) {
    target[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn set_runtime(runtime: &Arc<Mutex<RuntimeStatus>>, update: impl FnOnce(&mut RuntimeStatus)) {
    if let Ok(mut status) = runtime.lock() {
        update(&mut status);
    }
}

fn launch_stream<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    config: CameraConfig,
) -> Result<(), String> {
    config.validate()?;
    if let Some(previous) = state
        .stream
        .lock()
        .map_err(|_| "내부 상태 잠금 오류")?
        .take()
    {
        previous.stop();
    }
    let status = driver_status();
    let directshow_path = if config.backend == "directshow" {
        if !status.direct_show_installed {
            return Err("먼저 DirectShow 카메라 구성요소를 설치해 주세요.".into());
        }
        Some(native_file(app, "softcam64.dll")?)
    } else {
        if !status.media_foundation_installed {
            return Err("먼저 Windows 11 카메라 구성요소를 설치해 주세요.".into());
        }
        ensure_virtual_camera(app)?;
        None
    };
    let runtime = Arc::clone(&state.runtime);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    set_runtime(&runtime, |status| {
        status.streaming = false;
        status.connected = false;
        status.message = "가상 카메라를 시작하는 중…".into();
        status.last_error = None;
    });

    let handle = thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let mut writer = if config.backend == "media-foundation" {
                Some(SharedFrameWriter::open()?)
            } else {
                None
            };
            let directshow = directshow_path
                .as_deref()
                .map(|path| DirectShowCamera::open(path, &config))
                .transpose()?;
            let mut directshow_frame =
                Vec::with_capacity(config.width as usize * config.height as usize * 3);
            let mut static_frame = match config.source.as_str() {
                "image" => Some(load_image_frame(&config)?),
                "solid" => Some(solid_frame(&config)?),
                _ => None,
            };
            let mut generated_frame = vec![0_u8; (config.width * config.height * 4) as usize];
            let _ = ready_tx.send(Ok(()));
            set_runtime(&runtime, |status| {
                status.streaming = true;
                status.message = "카메라가 실행 중입니다".into();
            });

            let mut frame_index = 0_u64;
            let mut last_connection_check = Instant::now() - Duration::from_secs(1);
            let mut last_heartbeat = 0_u64;
            let mut heartbeat_seen_at = Instant::now() - Duration::from_secs(10);
            let frame_interval = Duration::from_secs_f64(1.0 / config.fps as f64);
            let mut next_frame = Instant::now();
            while !thread_stop.load(Ordering::Acquire) {
                let frame = if config.source == "screen-region" {
                    #[cfg(windows)]
                    {
                        generated_frame = screen_region_frame(&config)?;
                    }
                    generated_frame.as_slice()
                } else if let Some(frame) = static_frame.as_mut() {
                    frame.as_slice()
                } else {
                    test_pattern_frame(&config, frame_index, &mut generated_frame);
                    generated_frame.as_slice()
                };
                if let Some(writer) = writer.as_mut() {
                    writer.write_frame(&config, frame)?;
                } else if let Some(camera) = directshow.as_ref() {
                    camera.send_bgra(frame, &mut directshow_frame);
                }
                frame_index = frame_index.wrapping_add(1);
                if last_connection_check.elapsed() >= Duration::from_millis(500) {
                    let connected = if let Some(writer) = writer.as_ref() {
                        let heartbeat = writer.heartbeat();
                        if heartbeat != 0 && heartbeat != last_heartbeat {
                            last_heartbeat = heartbeat;
                            heartbeat_seen_at = Instant::now();
                        }
                        heartbeat_seen_at.elapsed() < Duration::from_secs(2)
                    } else {
                        directshow
                            .as_ref()
                            .map(DirectShowCamera::is_connected)
                            .unwrap_or(false)
                    };
                    set_runtime(&runtime, |status| {
                        status.connected = connected;
                        status.message = if connected {
                            "앱에서 카메라를 사용 중입니다".into()
                        } else {
                            "카메라가 실행 중이며 연결을 기다립니다".into()
                        };
                    });
                    last_connection_check = Instant::now();
                }
                next_frame += frame_interval;
                if let Some(remaining) = next_frame.checked_duration_since(Instant::now()) {
                    thread::sleep(remaining);
                } else {
                    next_frame = Instant::now();
                }
            }
            if let Some(writer) = writer.as_mut() {
                writer.set_inactive();
            }
            Ok(())
        })();

        if let Err(error) = result {
            let _ = ready_tx.send(Err(error.clone()));
            set_runtime(&runtime, |status| {
                status.last_error = Some(error.clone());
                status.message = error;
            });
        }
        set_runtime(&runtime, |status| {
            status.streaming = false;
            status.connected = false;
            if status.last_error.is_none() {
                status.message = "카메라가 꺼져 있습니다".into();
            }
        });
    });

    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {
            *state.stream.lock().map_err(|_| "내부 상태 잠금 오류")? = Some(StreamHandle {
                stop,
                thread: Some(handle),
            });
            Ok(())
        }
        Ok(Err(error)) => {
            let _ = handle.join();
            Err(error)
        }
        Err(_) => {
            stop.store(true, Ordering::Release);
            let _ = handle.join();
            Err("가상 카메라 시작 시간이 초과되었습니다.".into())
        }
    }
}

fn stop_stream(state: &AppState) -> Result<(), String> {
    if let Some(stream) = state
        .stream
        .lock()
        .map_err(|_| "내부 상태 잠금 오류")?
        .take()
    {
        stream.stop();
    }
    set_runtime(&state.runtime, |status| {
        status.streaming = false;
        status.connected = false;
        status.message = "카메라가 꺼져 있습니다".into();
        status.last_error = None;
    });
    Ok(())
}

#[tauri::command]
fn get_initial_state(app: AppHandle, state: State<'_, AppState>) -> Result<InitialState, String> {
    let settings = state
        .settings
        .lock()
        .map_err(|_| "내부 상태 잠금 오류")?
        .clone();
    let runtime = state
        .runtime
        .lock()
        .map_err(|_| "내부 상태 잠금 오류")?
        .clone();
    let autostart = app
        .autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())?;
    Ok(InitialState {
        settings,
        runtime,
        driver: driver_status(),
        autostart,
    })
}

#[tauri::command]
fn get_runtime_status(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    state
        .runtime
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "내부 상태 잠금 오류".into())
}

#[tauri::command]
fn get_driver_status() -> DriverStatus {
    driver_status()
}

#[tauri::command]
fn start_camera(
    app: AppHandle,
    state: State<'_, AppState>,
    config: CameraConfig,
    start_stream_on_launch: bool,
) -> Result<RuntimeStatus, String> {
    launch_stream(&app, &state, config.clone())?;
    let settings = Settings {
        camera: config,
        start_stream_on_launch,
    };
    save_settings(&app, &settings)?;
    *state.settings.lock().map_err(|_| "내부 상태 잠금 오류")? = settings;
    state
        .runtime
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "내부 상태 잠금 오류".into())
}

#[tauri::command]
fn stop_camera(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    stop_stream(&state)?;
    state
        .runtime
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "내부 상태 잠금 오류".into())
}

#[tauri::command]
fn save_preferences(
    app: AppHandle,
    state: State<'_, AppState>,
    config: CameraConfig,
    start_stream_on_launch: bool,
) -> Result<(), String> {
    config.validate()?;
    let settings = Settings {
        camera: config,
        start_stream_on_launch,
    };
    save_settings(&app, &settings)?;
    *state.settings.lock().map_err(|_| "내부 상태 잠금 오류")? = settings;
    Ok(())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|error| error.to_string())?;
    } else {
        manager.disable().map_err(|error| error.to_string())?;
    }
    manager.is_enabled().map_err(|error| error.to_string())
}

#[tauri::command]
async fn choose_image() -> Result<Option<SelectedImage>, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<Option<SelectedImage>, String> {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Image", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        else {
            return Ok(None);
        };
        let image =
            image::open(&path).map_err(|error| format!("이미지를 열 수 없습니다: {error}"))?;
        let preview = image.thumbnail(960, 540);
        let mut bytes = Vec::new();
        preview
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )
            .map_err(|error| format!("미리보기를 만들 수 없습니다: {error}"))?;
        Ok(Some(SelectedImage {
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .to_string(),
            path: path.to_string_lossy().into_owned(),
            preview_data_url: format!("data:image/jpeg;base64,{}", BASE64.encode(bytes)),
        }))
    })
    .await
    .map_err(|error| format!("파일 선택 작업이 중단됐습니다: {error}"))?
}

#[tauri::command]
async fn capture_desktop_preview() -> Result<DesktopPreview, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<DesktopPreview, String> {
        #[cfg(not(windows))]
        return Err("화면 영역 캡처는 Windows 11에서만 지원됩니다.".into());

        #[cfg(windows)]
        {
            let desktop = virtual_screen_region()?;
            let scale = (1200.0_f64 / desktop.width as f64)
                .min(700.0_f64 / desktop.height as f64)
                .min(1.0);
            let preview_width = (desktop.width as f64 * scale).round().max(1.0) as u32;
            let preview_height = (desktop.height as f64 * scale).round().max(1.0) as u32;
            let mut bgra = capture_screen_bgra(desktop, preview_width, preview_height)?;
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            let rgba = image::RgbaImage::from_raw(preview_width, preview_height, bgra)
                .ok_or_else(|| "화면 미리보기 이미지를 만들 수 없습니다.".to_string())?;
            let mut bytes = Vec::new();
            DynamicImage::ImageRgba8(rgba)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Jpeg,
                )
                .map_err(|error| format!("화면 미리보기를 인코딩할 수 없습니다: {error}"))?;
            Ok(DesktopPreview {
                preview_data_url: format!("data:image/jpeg;base64,{}", BASE64.encode(bytes)),
                virtual_x: desktop.x,
                virtual_y: desktop.y,
                virtual_width: desktop.width,
                virtual_height: desktop.height,
                preview_width,
                preview_height,
            })
        }
    })
    .await
    .map_err(|error| format!("화면 캡처 작업이 중단됐습니다: {error}"))?
}

#[tauri::command]
async fn manage_driver(
    app: AppHandle,
    backend: String,
    install: bool,
) -> Result<DriverStatus, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<DriverStatus, String> {
        let (manager, parameters, label) = if backend == "media-foundation" {
            let manager = native_file(&app, "mfvcam_manager.exe")?;
            let parameters = if install {
                let media_source = native_file(&app, "RustVirtualCameraMediaSource.dll")?;
                format!("install \"{}\"", media_source.display())
            } else {
                "uninstall".to_string()
            };
            (manager, parameters, "Windows 11")
        } else if backend == "directshow" {
            let manager = native_file(&app, "dshow_manager64.exe")?;
            let helper32 = native_file(&app, "dshow_manager32.exe")?;
            let parameters = if install {
                let dll64 = native_file(&app, "softcam64.dll")?;
                let dll32 = native_file(&app, "softcam32.dll")?;
                format!(
                    "install \"{}\" \"{}\" \"{}\"",
                    dll64.display(),
                    dll32.display(),
                    helper32.display()
                )
            } else {
                format!("uninstall \"{}\"", helper32.display())
            };
            (manager, parameters, "DirectShow")
        } else {
            return Err("지원하지 않는 카메라 출력 방식입니다.".into());
        };
        run_elevated(&manager, &parameters)
            .map_err(|error| format!("{label} 가상 카메라 작업 실패: {error}"))?;
        Ok(driver_status())
    })
    .await
    .map_err(|error| format!("드라이버 작업이 중단됐습니다: {error}"))?
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "열기", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let icon = app
        .default_window_icon()
        .cloned()
        .expect("application icon");
    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("Rust Virtual Camera")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            app.handle().plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                Some(vec!["--minimized"]),
            ))?;
            if cfg!(debug_assertions) {
                // A concurrently installed copy can briefly hold the log file.
                // Logging is diagnostic only, so it must not prevent startup.
                let _ = app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                );
            }
            create_tray(app.handle())?;
            let settings = load_settings(app.handle());
            let state = app.state::<AppState>();
            *state.settings.lock().expect("settings state") = settings.clone();
            set_runtime(&state.runtime, |status| {
                status.message = "카메라가 꺼져 있습니다".into()
            });
            if driver_status().media_foundation_installed {
                if let Err(error) = ensure_virtual_camera(app.handle()) {
                    log::warn!("{error}");
                }
            }
            if std::env::args().any(|argument| argument == "--minimized") {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            if settings.start_stream_on_launch {
                if let Err(error) = launch_stream(app.handle(), &state, settings.camera) {
                    set_runtime(&state.runtime, |status| {
                        status.message = error.clone();
                        status.last_error = Some(error);
                    });
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_initial_state,
            get_runtime_status,
            get_driver_status,
            start_camera,
            stop_camera,
            save_preferences,
            set_autostart,
            choose_image,
            capture_desktop_preview,
            manage_driver
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn captures_virtual_desktop_into_bgra_frame() {
        let desktop = virtual_screen_region().expect("virtual desktop bounds");
        let frame = capture_screen_bgra(desktop, 64, 36).expect("desktop capture");
        assert_eq!(frame.len(), 64 * 36 * 4);
        assert!(frame.chunks_exact(4).any(|pixel| pixel[0..3] != [0, 0, 0]));
    }

    #[test]
    fn directshow_sender_accepts_bgra_frame() {
        let dll = development_native_path("softcam/x64/Release/softcam.dll");
        let mut config = CameraConfig::default();
        config.backend = "directshow".into();
        config.width = 640;
        config.height = 480;
        let camera = DirectShowCamera::open(&dll, &config).expect("DirectShow sender");
        let frame = solid_frame(&config).expect("solid BGRA frame");
        let mut bgr = Vec::new();
        camera.send_bgra(&frame, &mut bgr);
        assert_eq!(bgr.len(), 640 * 480 * 3);
    }
}
