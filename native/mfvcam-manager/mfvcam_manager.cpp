#include <windows.h>
#include <sddl.h>
#include <aclapi.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mfvirtualcamera.h>
#include <shlobj.h>

#include <iostream>
#include <string>

#pragma comment(lib, "advapi32.lib")
#pragma comment(lib, "mfplat.lib")
#pragma comment(lib, "mfsensorgroup.lib")
#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "shell32.lib")

namespace
{
    constexpr wchar_t MEDIA_SOURCE_CLSID[] =
        L"{CD31FFCF-F7BE-42DC-A072-F49AD0E66AF7}";
    constexpr wchar_t CAMERA_NAME[] = L"Rust Virtual Camera";
    constexpr DWORD SHARED_FRAME_MAGIC = 0x4D435652;
    constexpr DWORD SHARED_FRAME_VERSION = 1;
    constexpr DWORD SHARED_FRAME_HEADER_SIZE = 64;
    constexpr DWORD SHARED_FRAME_MAX_WIDTH = 3840;
    constexpr DWORD SHARED_FRAME_MAX_HEIGHT = 2160;
    constexpr ULONGLONG SHARED_FRAME_FILE_SIZE =
        SHARED_FRAME_HEADER_SIZE +
        static_cast<ULONGLONG>(SHARED_FRAME_MAX_WIDTH) * SHARED_FRAME_MAX_HEIGHT * 4;

    // {C7F7C57B-DF30-41D0-AFFC-15201CDF920D}
    const GUID VCAM_KIND =
        { 0xc7f7c57b, 0xdf30, 0x41d0, { 0xaf, 0xfc, 0x15, 0x20, 0x1c, 0xdf, 0x92, 0x0d } };

    struct ComScope
    {
        HRESULT result = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
        ~ComScope()
        {
            if (SUCCEEDED(result))
            {
                CoUninitialize();
            }
        }
    };

    std::wstring ProgramDataDirectory()
    {
        PWSTR rawPath = nullptr;
        if (FAILED(SHGetKnownFolderPath(FOLDERID_ProgramData, 0, nullptr, &rawPath)))
        {
            return {};
        }
        std::wstring path(rawPath);
        CoTaskMemFree(rawPath);
        path += L"\\RustVirtualCamera";
        return path;
    }

    HRESULT ApplySharedAccess(const std::wstring& path)
    {
        PSECURITY_DESCRIPTOR descriptor = nullptr;
        if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(
            L"D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1301bf;;;BU)(A;OICI;0x1200a9;;;LS)",
            SDDL_REVISION_1,
            &descriptor,
            nullptr))
        {
            return HRESULT_FROM_WIN32(GetLastError());
        }

        BOOL present = FALSE;
        BOOL defaulted = FALSE;
        PACL dacl = nullptr;
        HRESULT result = E_FAIL;
        if (GetSecurityDescriptorDacl(descriptor, &present, &dacl, &defaulted) && present)
        {
            const DWORD error = SetNamedSecurityInfoW(
                const_cast<LPWSTR>(path.c_str()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                nullptr,
                nullptr,
                dacl,
                nullptr);
            result = HRESULT_FROM_WIN32(error);
        }
        else
        {
            result = HRESULT_FROM_WIN32(GetLastError());
        }
        LocalFree(descriptor);
        return result;
    }

    HRESULT PrepareSharedFrame(const std::wstring& directory)
    {
        const std::wstring path = directory + L"\\frame.bin";
        HANDLE file = CreateFileW(
            path.c_str(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            nullptr,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            nullptr);
        if (file == INVALID_HANDLE_VALUE)
        {
            return HRESULT_FROM_WIN32(GetLastError());
        }

        LARGE_INTEGER size{};
        size.QuadPart = SHARED_FRAME_FILE_SIZE;
        HRESULT result = S_OK;
        if (!SetFilePointerEx(file, size, nullptr, FILE_BEGIN) || !SetEndOfFile(file))
        {
            result = HRESULT_FROM_WIN32(GetLastError());
        }

        HANDLE mapping = nullptr;
        BYTE* view = nullptr;
        if (SUCCEEDED(result))
        {
            mapping = CreateFileMappingW(file, nullptr, PAGE_READWRITE, 0, 0, nullptr);
            if (mapping == nullptr)
            {
                result = HRESULT_FROM_WIN32(GetLastError());
            }
        }
        if (SUCCEEDED(result))
        {
            view = static_cast<BYTE*>(MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, SHARED_FRAME_HEADER_SIZE));
            if (view == nullptr)
            {
                result = HRESULT_FROM_WIN32(GetLastError());
            }
        }
        if (SUCCEEDED(result))
        {
            ZeroMemory(view, SHARED_FRAME_HEADER_SIZE);
            memcpy(view + 0, &SHARED_FRAME_MAGIC, sizeof(SHARED_FRAME_MAGIC));
            memcpy(view + 4, &SHARED_FRAME_VERSION, sizeof(SHARED_FRAME_VERSION));
            FlushViewOfFile(view, SHARED_FRAME_HEADER_SIZE);
        }

        if (view != nullptr) UnmapViewOfFile(view);
        if (mapping != nullptr) CloseHandle(mapping);
        CloseHandle(file);
        if (FAILED(result)) return result;
        return ApplySharedAccess(path);
    }

    HRESULT RegisterMediaSource(const std::wstring& dllPath)
    {
        const std::wstring keyPath =
            std::wstring(L"SOFTWARE\\Classes\\CLSID\\") +
            MEDIA_SOURCE_CLSID +
            L"\\InprocServer32";
        HKEY key = nullptr;
        const LSTATUS createResult = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            keyPath.c_str(),
            0,
            nullptr,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            nullptr,
            &key,
            nullptr);
        if (createResult != ERROR_SUCCESS)
        {
            return HRESULT_FROM_WIN32(createResult);
        }

        const wchar_t threadingModel[] = L"Both";
        LSTATUS result = RegSetValueExW(
            key,
            nullptr,
            0,
            REG_SZ,
            reinterpret_cast<const BYTE*>(dllPath.c_str()),
            static_cast<DWORD>((dllPath.size() + 1) * sizeof(wchar_t)));
        if (result == ERROR_SUCCESS)
        {
            result = RegSetValueExW(
                key,
                L"ThreadingModel",
                0,
                REG_SZ,
                reinterpret_cast<const BYTE*>(threadingModel),
                sizeof(threadingModel));
        }
        RegCloseKey(key);
        return HRESULT_FROM_WIN32(result);
    }

    HRESULT DeleteMediaSourceRegistration()
    {
        const std::wstring keyPath =
            std::wstring(L"SOFTWARE\\Classes\\CLSID\\") + MEDIA_SOURCE_CLSID;
        const LSTATUS result = RegDeleteTreeW(HKEY_LOCAL_MACHINE, keyPath.c_str());
        return result == ERROR_FILE_NOT_FOUND ? S_OK : HRESULT_FROM_WIN32(result);
    }

    HRESULT StartVirtualCamera()
    {
        ComScope com;
        if (FAILED(com.result) && com.result != RPC_E_CHANGED_MODE)
        {
            return com.result;
        }
        HRESULT result = MFStartup(MF_VERSION);
        if (FAILED(result)) return result;

        IMFVirtualCamera* camera = nullptr;
        result = MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_System,
            MFVirtualCameraAccess_CurrentUser,
            CAMERA_NAME,
            MEDIA_SOURCE_CLSID,
            nullptr,
            0,
            &camera);
        if (SUCCEEDED(result)) result = camera->SetUINT32(VCAM_KIND, 0);
        if (SUCCEEDED(result)) result = camera->Start(nullptr);
        if (camera != nullptr)
        {
            camera->Shutdown();
            camera->Release();
        }
        MFShutdown();
        return result;
    }

    HRESULT RemoveVirtualCamera()
    {
        ComScope com;
        if (FAILED(com.result) && com.result != RPC_E_CHANGED_MODE)
        {
            return com.result;
        }
        HRESULT result = MFStartup(MF_VERSION);
        if (FAILED(result)) return result;

        IMFVirtualCamera* camera = nullptr;
        result = MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_System,
            MFVirtualCameraAccess_CurrentUser,
            CAMERA_NAME,
            MEDIA_SOURCE_CLSID,
            nullptr,
            0,
            &camera);
        if (SUCCEEDED(result)) result = camera->Remove();
        if (camera != nullptr)
        {
            camera->Shutdown();
            camera->Release();
        }
        MFShutdown();
        return result;
    }

    HRESULT Install(const wchar_t* sourceDll)
    {
        const std::wstring directory = ProgramDataDirectory();
        if (directory.empty()) return E_FAIL;
        if (!CreateDirectoryW(directory.c_str(), nullptr) && GetLastError() != ERROR_ALREADY_EXISTS)
        {
            return HRESULT_FROM_WIN32(GetLastError());
        }
        HRESULT result = ApplySharedAccess(directory);
        if (FAILED(result)) return result;

        // Install side-by-side so an older media source still loaded by Windows
        // Frame Server does not block an application upgrade.
        const std::wstring installedDll = directory + L"\\RustVirtualCameraMediaSource-0.3.0.dll";
        if (!CopyFileW(sourceDll, installedDll.c_str(), FALSE))
        {
            return HRESULT_FROM_WIN32(GetLastError());
        }
        result = PrepareSharedFrame(directory);
        if (FAILED(result)) return result;
        result = RegisterMediaSource(installedDll);
        if (FAILED(result)) return result;
        result = StartVirtualCamera();
        return result;
    }

    HRESULT Uninstall()
    {
        HRESULT result = RemoveVirtualCamera();
        const HRESULT registryResult = DeleteMediaSourceRegistration();
        if (SUCCEEDED(result)) result = registryResult;
        const std::wstring directory = ProgramDataDirectory();
        if (!directory.empty())
        {
            const std::wstring dllPath = directory + L"\\RustVirtualCameraMediaSource-0.3.0.dll";
            const std::wstring previousDllPath = directory + L"\\RustVirtualCameraMediaSource-0.2.0.dll";
            const std::wstring legacyDllPath = directory + L"\\RustVirtualCameraMediaSource.dll";
            const std::wstring framePath = directory + L"\\frame.bin";
            if (!DeleteFileW(dllPath.c_str()))
            {
                MoveFileExW(dllPath.c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT);
            }
            if (!DeleteFileW(legacyDllPath.c_str()))
            {
                MoveFileExW(legacyDllPath.c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT);
            }
            if (!DeleteFileW(previousDllPath.c_str()))
            {
                MoveFileExW(previousDllPath.c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT);
            }
            DeleteFileW(framePath.c_str());
            RemoveDirectoryW(directory.c_str());
        }
        return result;
    }

    void PrintError(HRESULT result)
    {
        wchar_t* message = nullptr;
        FormatMessageW(
            FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            nullptr,
            result,
            0,
            reinterpret_cast<LPWSTR>(&message),
            0,
            nullptr);
        std::wcerr << L"HRESULT 0x" << std::hex << static_cast<unsigned long>(result);
        if (message != nullptr)
        {
            std::wcerr << L": " << message;
            LocalFree(message);
        }
        std::wcerr << std::endl;
    }
}

int wmain(int argc, wchar_t* argv[])
{
    if (argc < 2)
    {
        std::wcerr << L"Usage: mfvcam_manager install <media-source.dll> | ensure | uninstall" << std::endl;
        return 2;
    }

    HRESULT result = E_INVALIDARG;
    if (_wcsicmp(argv[1], L"install") == 0 && argc == 3)
    {
        result = Install(argv[2]);
    }
    else if (_wcsicmp(argv[1], L"ensure") == 0)
    {
        result = StartVirtualCamera();
    }
    else if (_wcsicmp(argv[1], L"uninstall") == 0)
    {
        result = Uninstall();
    }

    if (FAILED(result))
    {
        PrintError(result);
        return static_cast<int>(result & 0x7FFFFFFF);
    }
    return 0;
}
