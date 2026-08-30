const { invoke } = window.__TAURI__.core;

const elements = {
  statusPill: document.querySelector("#status-pill"),
  statusText: document.querySelector("#status-text"),
  connectionText: document.querySelector("#connection-text"),
  preview: document.querySelector("#preview"),
  previewOff: document.querySelector("#preview-off"),
  previewLabel: document.querySelector("#preview-label"),
  resolutionBadge: document.querySelector("#resolution-badge"),
  backendButtons: [...document.querySelectorAll("[data-backend]")],
  sourceButtons: [...document.querySelectorAll("[data-source]")],
  resolution: document.querySelector("#resolution"),
  fps: document.querySelector("#fps"),
  mirror: document.querySelector("#mirror"),
  color: document.querySelector("#solid-color"),
  colorValue: document.querySelector("#color-value"),
  imageButton: document.querySelector("#image-button"),
  imageName: document.querySelector("#image-name"),
  regionButton: document.querySelector("#region-button"),
  regionSummary: document.querySelector("#region-summary"),
  regionModal: document.querySelector("#region-modal"),
  regionPreview: document.querySelector("#region-preview"),
  regionPreviewImage: document.querySelector("#region-preview-image"),
  regionSelection: document.querySelector("#region-selection"),
  regionCancel: document.querySelector("#region-cancel"),
  regionConfirm: document.querySelector("#region-confirm"),
  autostart: document.querySelector("#autostart"),
  startOnLaunch: document.querySelector("#start-on-launch"),
  cameraButton: document.querySelector("#camera-button"),
  cameraButtonText: document.querySelector("#camera-button-text"),
  driverButton: document.querySelector("#driver-button"),
  driverTitle: document.querySelector("#driver-title"),
  driverText: document.querySelector("#driver-text"),
  feedback: document.querySelector("#feedback"),
};

const app = {
  backend: "media-foundation",
  source: "test-pattern",
  imagePath: null,
  imagePreview: null,
  captureRegion: null,
  screenPreview: null,
  desktopPreview: null,
  selection: null,
  runtime: { streaming: false, connected: false, message: "준비 중…", lastError: null },
  driver: { mediaFoundationInstalled: false, directShowInstalled: false },
  previewFrame: 0,
};

function setFeedback(message = "", error = false) {
  elements.feedback.textContent = message;
  elements.feedback.classList.toggle("error", error);
}

function currentConfig() {
  const [width, height] = elements.resolution.value.split("x").map(Number);
  return {
    backend: app.backend,
    width,
    height,
    fps: Number(elements.fps.value),
    source: app.source,
    imagePath: app.imagePath,
    color: elements.color.value,
    mirror: elements.mirror.checked,
    captureRegion: app.captureRegion,
  };
}

function applyConfig(config) {
  app.backend = config.backend || "media-foundation";
  app.source = config.source;
  app.imagePath = config.imagePath;
  app.captureRegion = config.captureRegion || null;
  elements.resolution.value = `${config.width}x${config.height}`;
  elements.fps.value = String(config.fps);
  elements.mirror.checked = config.mirror;
  elements.color.value = config.color;
  elements.colorValue.textContent = config.color.toUpperCase();
  if (config.imagePath) elements.imageName.textContent = config.imagePath.split(/[\\/]/).pop();
  updateRegionSummary();
  selectBackend(app.backend);
  selectSource(config.source);
}

function selectedDriverInstalled() {
  return app.backend === "directshow"
    ? app.driver.directShowInstalled
    : app.driver.mediaFoundationInstalled;
}

function selectBackend(backend) {
  app.backend = backend;
  elements.backendButtons.forEach((button) => {
    button.classList.toggle("active", button.dataset.backend === backend);
  });
  renderDriver(app.driver);
}

function selectSource(source) {
  app.source = source;
  elements.sourceButtons.forEach((button) => {
    button.classList.toggle("active", button.dataset.source === source);
  });
  ["test-pattern", "solid", "image", "screen-region"].forEach((name) => {
    document.querySelector(`#${name}-panel`).classList.toggle("hidden", name !== source);
  });
  elements.previewLabel.textContent = {
    "test-pattern": "테스트 패턴",
    solid: "단색 화면",
    image: "이미지",
    "screen-region": "화면 영역",
  }[source];
  updatePreviewMetadata();
}

function updateRegionSummary() {
  const region = app.captureRegion;
  elements.regionSummary.textContent = region
    ? `X ${region.x}, Y ${region.y} · ${region.width} × ${region.height}`
    : "바탕 화면에서 영역을 지정하세요";
}

function updatePreviewMetadata() {
  const config = currentConfig();
  elements.resolutionBadge.textContent = `${config.width} × ${config.height} · ${config.fps} FPS`;
}

function renderRuntime(runtime) {
  app.runtime = runtime;
  const state = runtime.lastError
    ? "error"
    : runtime.connected
      ? "connected"
      : runtime.streaming
        ? "on"
        : "off";
  elements.statusPill.dataset.state = state;
  elements.statusText.textContent = runtime.message || "카메라가 꺼져 있습니다";
  elements.connectionText.textContent = runtime.connected
    ? "카메라 사용 앱 연결됨"
    : runtime.streaming
      ? "연결된 앱을 기다리는 중"
      : "연결된 앱 없음";
  elements.previewOff.classList.toggle("hidden", runtime.streaming);
  elements.cameraButton.classList.toggle("stop", runtime.streaming);
  elements.cameraButtonText.textContent = runtime.streaming ? "카메라 중지" : "카메라 시작";
  if (runtime.lastError) setFeedback(runtime.lastError, true);
}

function renderDriver(driver) {
  app.driver = driver;
  const directShow = app.backend === "directshow";
  const installed = selectedDriverInstalled();
  elements.driverTitle.textContent = directShow
    ? "DirectShow 가상 카메라"
    : "Windows 11 가상 카메라";
  elements.driverText.textContent = installed
    ? directShow
      ? "레거시 DirectShow 기반 앱에서 사용할 수 있습니다."
      : "Windows 카메라, Discord, Teams 등에서 사용할 수 있습니다."
    : directShow
      ? "DirectShow x64/x86 구성요소를 설치해 주세요."
      : "Windows 11 Media Foundation 구성요소를 설치해 주세요.";
  elements.driverButton.textContent = installed ? "구성요소 제거" : "구성요소 설치";
  elements.driverButton.dataset.action = installed ? "remove" : "install";
}

function drawCoverImage(ctx, image, width, height, mirrored) {
  const scale = Math.max(width / image.naturalWidth, height / image.naturalHeight);
  const drawWidth = image.naturalWidth * scale;
  const drawHeight = image.naturalHeight * scale;
  ctx.save();
  if (mirrored) {
    ctx.translate(width, 0);
    ctx.scale(-1, 1);
  }
  ctx.drawImage(image, (width - drawWidth) / 2, (height - drawHeight) / 2, drawWidth, drawHeight);
  ctx.restore();
}

function drawPreview() {
  const canvas = elements.preview;
  const ctx = canvas.getContext("2d");
  const { width, height, mirror, color } = currentConfig();
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }

  if (app.source === "solid") {
    ctx.fillStyle = color;
    ctx.fillRect(0, 0, width, height);
  } else if (app.source === "image" && app.imagePreview?.complete) {
    ctx.fillStyle = "#09090b";
    ctx.fillRect(0, 0, width, height);
    drawCoverImage(ctx, app.imagePreview, width, height, mirror);
  } else if (app.source === "image") {
    ctx.fillStyle = "#111116";
    ctx.fillRect(0, 0, width, height);
  } else if (app.source === "screen-region" && app.screenPreview?.complete && app.desktopPreview && app.captureRegion) {
    const desktop = app.desktopPreview;
    const region = app.captureRegion;
    const scaleX = desktop.previewWidth / desktop.virtualWidth;
    const scaleY = desktop.previewHeight / desktop.virtualHeight;
    const sx = (region.x - desktop.virtualX) * scaleX;
    const sy = (region.y - desktop.virtualY) * scaleY;
    const sw = region.width * scaleX;
    const sh = region.height * scaleY;
    ctx.save();
    if (mirror) {
      ctx.translate(width, 0);
      ctx.scale(-1, 1);
    }
    ctx.drawImage(app.screenPreview, sx, sy, sw, sh, 0, 0, width, height);
    ctx.restore();
  } else if (app.source === "screen-region") {
    ctx.fillStyle = "#111116";
    ctx.fillRect(0, 0, width, height);
  } else {
    const colors = ["#ebebeb", "#ebeb10", "#10ebeb", "#10eb10", "#eb10eb", "#eb1010", "#1010eb", "#141418"];
    const barWidth = width / colors.length;
    colors.forEach((bar, index) => {
      ctx.fillStyle = bar;
      const x = mirror ? width - (index + 1) * barWidth : index * barWidth;
      ctx.fillRect(x, 0, Math.ceil(barWidth), height * 0.75);
    });
    const tile = Math.max(24, width / 20);
    for (let y = height * 0.75; y < height; y += tile) {
      for (let x = 0; x < width; x += tile) {
        ctx.fillStyle = ((x / tile + y / tile) & 1) ? "#22242c" : "#0f1014";
        ctx.fillRect(x, y, tile, tile);
      }
    }
    const scanX = (app.previewFrame * 4) % width;
    ctx.fillStyle = "rgba(255,255,255,.86)";
    ctx.fillRect(mirror ? width - scanX : scanX, 0, Math.max(3, width / 200), height);
  }
  app.previewFrame += 1;
  requestAnimationFrame(drawPreview);
}

async function updatePreferences({ applyToStream = false } = {}) {
  try {
    const preferences = {
      config: currentConfig(),
      startStreamOnLaunch: elements.startOnLaunch.checked,
    };
    if (applyToStream && app.runtime.streaming) {
      renderRuntime(await invoke("start_camera", preferences));
      setFeedback("변경한 설정이 실행 중인 카메라에 적용됐습니다.");
    } else {
      await invoke("save_preferences", preferences);
    }
    return true;
  } catch (error) {
    setFeedback(String(error), true);
    return false;
  }
}

async function toggleCamera() {
  elements.cameraButton.disabled = true;
  setFeedback();
  try {
    if (app.runtime.streaming) {
      renderRuntime(await invoke("stop_camera"));
    } else {
      if (!selectedDriverInstalled()) {
        throw new Error(`먼저 ${app.backend === "directshow" ? "DirectShow" : "Windows 11"} 가상 카메라 구성요소를 설치해 주세요.`);
      }
      renderRuntime(await invoke("start_camera", {
        config: currentConfig(),
        startStreamOnLaunch: elements.startOnLaunch.checked,
      }));
      setFeedback(app.backend === "directshow"
        ? "다른 앱의 카메라 목록에서 ‘Rust Virtual Camera (DirectShow)’를 선택하세요."
        : "다른 앱의 카메라 목록에서 ‘Rust Virtual Camera’를 선택하세요.");
    }
  } catch (error) {
    setFeedback(String(error), true);
  } finally {
    elements.cameraButton.disabled = false;
  }
}

async function initialize() {
  try {
    const initial = await invoke("get_initial_state");
    applyConfig(initial.settings.camera);
    elements.startOnLaunch.checked = initial.settings.startStreamOnLaunch;
    elements.autostart.checked = initial.autostart;
    renderRuntime(initial.runtime);
    renderDriver(initial.driver);
  } catch (error) {
    setFeedback(String(error), true);
    elements.statusText.textContent = "초기화 실패";
    elements.statusPill.dataset.state = "error";
  }
}

elements.sourceButtons.forEach((button) => button.addEventListener("click", async () => {
  if (button.dataset.source === app.source) return;
  selectSource(button.dataset.source);
  await updatePreferences({ applyToStream: true });
}));

elements.backendButtons.forEach((button) => button.addEventListener("click", async () => {
  try {
    if (button.dataset.backend === app.backend) return;
    const wasStreaming = app.runtime.streaming;
    if (wasStreaming) {
      renderRuntime(await invoke("stop_camera"));
    }
    selectBackend(button.dataset.backend);
    if (wasStreaming && selectedDriverInstalled()) {
      renderRuntime(await invoke("start_camera", {
        config: currentConfig(),
        startStreamOnLaunch: elements.startOnLaunch.checked,
      }));
    } else {
      await updatePreferences();
    }
    if (wasStreaming && !selectedDriverInstalled()) {
      setFeedback(`선택한 출력을 사용하려면 먼저 ${app.backend === "directshow" ? "DirectShow" : "Windows 11"} 구성요소를 설치해 주세요.`);
      return;
    }
    setFeedback(app.backend === "directshow"
      ? `DirectShow 출력이 선택됐습니다${wasStreaming ? "(스트림 재시작 완료)." : "."}`
      : `Windows 11 출력이 선택됐습니다${wasStreaming ? "(스트림 재시작 완료)." : "."}`);
  } catch (error) {
    setFeedback(String(error), true);
  }
}));

[elements.resolution, elements.fps, elements.mirror].forEach((element) => {
  element.addEventListener("change", () => {
    updatePreviewMetadata();
    updatePreferences({ applyToStream: true });
  });
});

elements.startOnLaunch.addEventListener("change", updatePreferences);

elements.color.addEventListener("input", () => {
  elements.colorValue.textContent = elements.color.value.toUpperCase();
});
elements.color.addEventListener("change", () => updatePreferences({ applyToStream: true }));

elements.imageButton.addEventListener("click", async () => {
  try {
    const selected = await invoke("choose_image");
    if (!selected) return;
    app.imagePath = selected.path;
    elements.imageName.textContent = selected.name;
    const image = new Image();
    image.src = selected.previewDataUrl;
    app.imagePreview = image;
    await updatePreferences({ applyToStream: true });
  } catch (error) {
    setFeedback(String(error), true);
  }
});

function selectionFromPointer(event) {
  const bounds = elements.regionPreviewImage.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(bounds.width, event.clientX - bounds.left)),
    y: Math.max(0, Math.min(bounds.height, event.clientY - bounds.top)),
    bounds,
  };
}

function renderSelection(selection) {
  if (!selection) {
    elements.regionSelection.style.display = "none";
    return;
  }
  elements.regionSelection.style.display = "block";
  elements.regionSelection.style.left = `${selection.x}px`;
  elements.regionSelection.style.top = `${selection.y}px`;
  elements.regionSelection.style.width = `${selection.width}px`;
  elements.regionSelection.style.height = `${selection.height}px`;
}

let selectionStart = null;
elements.regionPreview.addEventListener("pointerdown", (event) => {
  const point = selectionFromPointer(event);
  selectionStart = { x: point.x, y: point.y };
  app.selection = { x: point.x, y: point.y, width: 0, height: 0 };
  elements.regionPreview.setPointerCapture(event.pointerId);
  renderSelection(app.selection);
});
elements.regionPreview.addEventListener("pointermove", (event) => {
  if (!selectionStart) return;
  const point = selectionFromPointer(event);
  app.selection = {
    x: Math.min(selectionStart.x, point.x),
    y: Math.min(selectionStart.y, point.y),
    width: Math.abs(point.x - selectionStart.x),
    height: Math.abs(point.y - selectionStart.y),
  };
  renderSelection(app.selection);
});
elements.regionPreview.addEventListener("pointerup", () => {
  selectionStart = null;
});

elements.regionButton.addEventListener("click", async () => {
  elements.regionButton.disabled = true;
  setFeedback("데스크톱 미리보기를 캡처하는 중입니다…");
  try {
    const preview = await invoke("capture_desktop_preview");
    app.desktopPreview = preview;
    const image = new Image();
    image.src = preview.previewDataUrl;
    await image.decode();
    app.screenPreview = image;
    elements.regionPreviewImage.src = preview.previewDataUrl;
    elements.regionModal.classList.remove("hidden");
    requestAnimationFrame(() => {
      const bounds = elements.regionPreviewImage.getBoundingClientRect();
      if (app.captureRegion) {
        const scaleX = bounds.width / preview.virtualWidth;
        const scaleY = bounds.height / preview.virtualHeight;
        app.selection = {
          x: (app.captureRegion.x - preview.virtualX) * scaleX,
          y: (app.captureRegion.y - preview.virtualY) * scaleY,
          width: app.captureRegion.width * scaleX,
          height: app.captureRegion.height * scaleY,
        };
      } else {
        app.selection = { x: bounds.width * 0.1, y: bounds.height * 0.1, width: bounds.width * 0.8, height: bounds.height * 0.8 };
      }
      renderSelection(app.selection);
    });
    setFeedback();
  } catch (error) {
    setFeedback(String(error), true);
  } finally {
    elements.regionButton.disabled = false;
  }
});

elements.regionCancel.addEventListener("click", () => {
  elements.regionModal.classList.add("hidden");
});
elements.regionConfirm.addEventListener("click", async () => {
  const selection = app.selection;
  const preview = app.desktopPreview;
  const bounds = elements.regionPreviewImage.getBoundingClientRect();
  if (!selection || selection.width < 4 || selection.height < 4 || !preview) {
    setFeedback("송출할 영역을 조금 더 크게 드래그해 주세요.", true);
    return;
  }
  const scaleX = preview.virtualWidth / bounds.width;
  const scaleY = preview.virtualHeight / bounds.height;
  app.captureRegion = {
    x: preview.virtualX + Math.round(selection.x * scaleX),
    y: preview.virtualY + Math.round(selection.y * scaleY),
    width: Math.max(16, Math.round(selection.width * scaleX)),
    height: Math.max(16, Math.round(selection.height * scaleY)),
  };
  updateRegionSummary();
  elements.regionModal.classList.add("hidden");
  const applied = await updatePreferences({ applyToStream: true });
  if (applied) {
    setFeedback(app.runtime.streaming
      ? "선택한 화면 영역이 실행 중인 카메라에 적용됐습니다."
      : "선택한 화면 영역이 저장됐습니다.");
  }
});

elements.autostart.addEventListener("change", async () => {
  try {
    elements.autostart.checked = await invoke("set_autostart", { enabled: elements.autostart.checked });
  } catch (error) {
    elements.autostart.checked = !elements.autostart.checked;
    setFeedback(String(error), true);
  }
});

elements.driverButton.addEventListener("click", async () => {
  const install = elements.driverButton.dataset.action !== "remove";
  elements.driverButton.disabled = true;
  setFeedback(install ? "Windows 권한 확인 창을 승인해 주세요." : "드라이버를 제거하는 중입니다.");
  try {
    if (app.runtime.streaming) {
      renderRuntime(await invoke("stop_camera"));
    }
    renderDriver(await invoke("manage_driver", { backend: app.backend, install }));
    const label = app.backend === "directshow" ? "DirectShow" : "Windows 11";
    setFeedback(install ? `${label} 가상 카메라 설치가 완료됐습니다.` : `${label} 가상 카메라가 제거됐습니다.`);
  } catch (error) {
    setFeedback(String(error), true);
  } finally {
    elements.driverButton.disabled = false;
  }
});

elements.cameraButton.addEventListener("click", toggleCamera);

setInterval(async () => {
  try {
    renderRuntime(await invoke("get_runtime_status"));
  } catch (_) {
    // The app may be closing; no user action is required.
  }
}, 900);

drawPreview();
initialize();
