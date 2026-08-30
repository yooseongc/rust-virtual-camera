#include <windows.h>

#include <iostream>
#include <string>

namespace
{
    using RegistrationFunction = HRESULT(STDAPICALLTYPE*)();

    std::wstring ProgramDataDirectory()
    {
        wchar_t value[MAX_PATH]{};
        const DWORD length = GetEnvironmentVariableW(L"ProgramData", value, MAX_PATH);
        if (length == 0 || length >= MAX_PATH) return {};
        return std::wstring(value, length) + L"\\RustVirtualCamera";
    }

    HRESULT CallRegistration(const std::wstring& dllPath, const char* procedure)
    {
        HMODULE module = LoadLibraryW(dllPath.c_str());
        if (module == nullptr) return HRESULT_FROM_WIN32(GetLastError());
        auto function = reinterpret_cast<RegistrationFunction>(GetProcAddress(module, procedure));
        const HRESULT result = function == nullptr
            ? HRESULT_FROM_WIN32(GetLastError())
            : function();
        FreeLibrary(module);
        return result;
    }

    HRESULT RunHelper(
        const std::wstring& helper,
        const wchar_t* operation,
        const std::wstring& dllPath)
    {
        std::wstring command = L"\"" + helper + L"\" " + operation + L" \"" + dllPath + L"\"";
        STARTUPINFOW startup{};
        startup.cb = sizeof(startup);
        PROCESS_INFORMATION process{};
        if (!CreateProcessW(
                nullptr,
                command.data(),
                nullptr,
                nullptr,
                FALSE,
                CREATE_NO_WINDOW,
                nullptr,
                nullptr,
                &startup,
                &process))
        {
            return HRESULT_FROM_WIN32(GetLastError());
        }
        WaitForSingleObject(process.hProcess, INFINITE);
        DWORD exitCode = ERROR_GEN_FAILURE;
        GetExitCodeProcess(process.hProcess, &exitCode);
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        return exitCode == 0 ? S_OK : E_FAIL;
    }

    void DeleteOrSchedule(const std::wstring& path)
    {
        if (!DeleteFileW(path.c_str()) && GetLastError() != ERROR_FILE_NOT_FOUND)
        {
            MoveFileExW(path.c_str(), nullptr, MOVEFILE_DELAY_UNTIL_REBOOT);
        }
    }

    void CleanRegistrationView(REGSAM view)
    {
        HKEY classes = nullptr;
        if (RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                L"SOFTWARE\\Classes",
                0,
                KEY_READ | KEY_WRITE | view,
                &classes) != ERROR_SUCCESS)
        {
            return;
        }
        RegDeleteTreeW(classes, L"CLSID\\{AEF3B972-5FA5-4647-9571-358EB472BC9E}");
        RegDeleteTreeW(
            classes,
            L"CLSID\\{860BB310-5D01-11D0-BD3B-00A0C911CE86}\\Instance\\DirectShow Softcam");
        RegDeleteTreeW(
            classes,
            L"CLSID\\{860BB310-5D01-11D0-BD3B-00A0C911CE86}\\Instance\\Rust Virtual Camera (DirectShow)");
        RegCloseKey(classes);
    }

#ifdef _WIN64
    HRESULT Install(
        const std::wstring& source64,
        const std::wstring& source32,
        const std::wstring& helper32Source)
    {
        const std::wstring directory = ProgramDataDirectory();
        if (directory.empty()) return E_FAIL;
        if (!CreateDirectoryW(directory.c_str(), nullptr) && GetLastError() != ERROR_ALREADY_EXISTS)
        {
            return HRESULT_FROM_WIN32(GetLastError());
        }
        CleanRegistrationView(KEY_WOW64_64KEY);
        CleanRegistrationView(KEY_WOW64_32KEY);

        const std::wstring dll64 = directory + L"\\RustVirtualCameraDirectShow64-0.3.0.dll";
        const std::wstring dll32 = directory + L"\\RustVirtualCameraDirectShow32-0.3.0.dll";
        const std::wstring helper32 = directory + L"\\RustVirtualCameraDirectShowManager32-0.3.0.exe";
        if (!CopyFileW(source64.c_str(), dll64.c_str(), FALSE) ||
            !CopyFileW(source32.c_str(), dll32.c_str(), FALSE) ||
            !CopyFileW(helper32Source.c_str(), helper32.c_str(), FALSE))
        {
            return HRESULT_FROM_WIN32(GetLastError());
        }

        HRESULT result = CallRegistration(dll64, "DllRegisterServer");
        if (SUCCEEDED(result)) result = RunHelper(helper32, L"register", dll32);
        if (FAILED(result)) CallRegistration(dll64, "DllUnregisterServer");
        return result;
    }

    HRESULT Uninstall(const std::wstring& helper32Source)
    {
        const std::wstring directory = ProgramDataDirectory();
        if (directory.empty()) return E_FAIL;
        const std::wstring dll64 = directory + L"\\RustVirtualCameraDirectShow64-0.3.0.dll";
        const std::wstring dll32 = directory + L"\\RustVirtualCameraDirectShow32-0.3.0.dll";
        const std::wstring helper32 = directory + L"\\RustVirtualCameraDirectShowManager32-0.3.0.exe";

        HRESULT result = S_OK;
        if (GetFileAttributesW(dll64.c_str()) != INVALID_FILE_ATTRIBUTES)
        {
            result = CallRegistration(dll64, "DllUnregisterServer");
        }
        const std::wstring& helper = GetFileAttributesW(helper32.c_str()) != INVALID_FILE_ATTRIBUTES
            ? helper32
            : helper32Source;
        if (!helper.empty() && GetFileAttributesW(dll32.c_str()) != INVALID_FILE_ATTRIBUTES)
        {
            const HRESULT helperResult = RunHelper(helper, L"unregister", dll32);
            if (SUCCEEDED(result)) result = helperResult;
        }
        CleanRegistrationView(KEY_WOW64_64KEY);
        CleanRegistrationView(KEY_WOW64_32KEY);
        DeleteOrSchedule(dll64);
        DeleteOrSchedule(dll32);
        DeleteOrSchedule(helper32);
        return result;
    }
#endif
}

int wmain(int argc, wchar_t** argv)
{
    HRESULT result = E_INVALIDARG;
#ifdef _WIN64
    if (argc == 5 && std::wstring(argv[1]) == L"install")
    {
        result = Install(argv[2], argv[3], argv[4]);
    }
    else if (argc == 3 && std::wstring(argv[1]) == L"uninstall")
    {
        result = Uninstall(argv[2]);
    }
#else
    if (argc == 3 && std::wstring(argv[1]) == L"register")
    {
        result = CallRegistration(argv[2], "DllRegisterServer");
    }
    else if (argc == 3 && std::wstring(argv[1]) == L"unregister")
    {
        result = CallRegistration(argv[2], "DllUnregisterServer");
    }
#endif
    if (FAILED(result))
    {
        std::wcerr << L"HRESULT 0x" << std::hex << static_cast<unsigned long>(result) << L"\n";
        return 1;
    }
    return 0;
}
