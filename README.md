# Rust Virtual Camera

Windows 11에서 테스트 패턴, 단색 화면, 정지 이미지 또는 사용자가 지정한 화면 영역을 가상 카메라로 제공하는 Tauri 2 + Rust 데스크톱 앱입니다. Windows 기본 카메라 앱용 Media Foundation 출력과 레거시 앱용 DirectShow 출력을 선택할 수 있습니다.

## 기능

- 640×480, 1280×720, 1920×1080 출력과 24/30/60 FPS
- 테스트 패턴, 단색, 이미지 및 마우스로 지정한 화면 영역 소스
- 실시간 화면 영역 캡처와 좌우 반전
- Windows 11 `MFCreateVirtualCamera` 기반 가상 카메라 설치 및 제거
- x64/x86 앱을 위한 DirectShow 가상 카메라 설치 및 제거
- 실행 중 Media Foundation 또는 DirectShow 출력 선택
- Rust 앱과 Windows Camera Frame Server 사이의 메모리 매핑 프레임 전송
- Windows 로그인 시 자동 실행
- 창을 닫아도 스트림을 유지하는 시스템 트레이
- 마지막 설정 저장 및 앱 실행 시 스트림 자동 시작

## 개발 빌드

필수 도구는 Rust, Tauri CLI, Visual Studio 2022의 **Desktop development with C++** 워크로드와 Windows SDK 10.0.22621.0 이상입니다.

```powershell
# 네이티브 Media Foundation DLL과 설치 관리 도구
.\scripts\build-native.ps1

# Tauri 앱
cargo tauri dev

# NSIS와 MSI 설치 파일
cargo tauri build
```

Media Foundation 미디어 소스는 MIT 라이선스의 [Microsoft Windows-Camera VirtualCamera 샘플](https://github.com/microsoft/Windows-Camera/tree/master/Samples/VirtualCamera)을 기반으로 합니다. 첫 설치와 제거에는 관리자 권한이 필요하지만, 영상 송출과 재부팅 후 카메라 활성화는 일반 사용자 권한으로 동작합니다. 배포 전에는 앱 실행 파일과 네이티브 DLL을 코드 서명하는 것을 권장합니다.

> 이 프로젝트는 회의, 방송, 콘텐츠 제작 및 소프트웨어 테스트 같은 일반적인 영상 입력 용도입니다. 인증 또는 접근 통제 우회를 목적으로 사용하지 마세요.
