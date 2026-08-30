//
// Copyright (C) Microsoft Corporation. All rights reserved.
//
#include "pch.h"

namespace
{
    constexpr DWORD SHARED_FRAME_MAGIC = 0x4D435652; // "RVCM"
    constexpr DWORD SHARED_FRAME_VERSION = 1;
    constexpr DWORD SHARED_FRAME_HEADER_SIZE = 64;
    constexpr DWORD SHARED_FRAME_MAX_WIDTH = 3840;
    constexpr DWORD SHARED_FRAME_MAX_HEIGHT = 2160;
    constexpr ULONGLONG SHARED_FRAME_FILE_SIZE =
        SHARED_FRAME_HEADER_SIZE +
        static_cast<ULONGLONG>(SHARED_FRAME_MAX_WIDTH) * SHARED_FRAME_MAX_HEIGHT * 4;

    const wchar_t SHARED_FRAME_PATH[] =
        L"C:\\ProgramData\\RustVirtualCamera\\frame.bin";

    DWORD ReadU32(const BYTE* base, size_t offset)
    {
        DWORD value = 0;
        memcpy(&value, base + offset, sizeof(value));
        return value;
    }

    LONGLONG ReadSequence(const BYTE* base)
    {
        return InterlockedCompareExchange64(
            reinterpret_cast<volatile LONGLONG*>(const_cast<BYTE*>(base + 8)),
            0,
            0);
    }
}

SimpleFrameGenerator::~SimpleFrameGenerator()
{
    _CloseSharedFrame();
}

HRESULT SimpleFrameGenerator::Initialize(_In_ IMFMediaType* pMediaType)
{
    RETURN_HR_IF_NULL(E_INVALIDARG, pMediaType);

    RETURN_IF_FAILED(pMediaType->GetGUID(MF_MT_SUBTYPE, &m_subType));
    if (m_subType != MFVideoFormat_RGB32 && m_subType != MFVideoFormat_NV12)
    {
        RETURN_HR_MSG(MF_E_UNSUPPORTED_FORMAT, "Unsupported format: %s", winrt::to_hstring(m_subType).data());
    }
    MFGetAttributeSize(pMediaType, MF_MT_FRAME_SIZE, &m_width, &m_height);

    return S_OK;
}

/*:
   Writes to a buffer representing a 2D image.
   Writes a different constant to each line based on row number and current time.
   Assumes top down image, no negative stride and pBuf points to the begnning of the buffer of length len.
   Param:
   pBuf - pointer to beginning of buffer
   pitch - line length in bytes
   len - length of buffer in bytes
*/
HRESULT SimpleFrameGenerator::CreateFrame(
    _Inout_updates_bytes_(len) BYTE* pBuf,
    _In_ DWORD len,
    _In_ LONG pitch,
    _In_ ULONG rgbMask)
{
    if (m_subType == MFVideoFormat_RGB32)
    {
        DEBUG_MSG(L"RGB32 frames %s\n", winrt::to_hstring(MFVideoFormat_RGB32).data());

        // RGB DIB frames use bottom-up scan lines unless a negative stride is negotiated.
        RETURN_IF_FAILED(_CreateRGB32Frame(pBuf, len, pitch, m_width, m_height, rgbMask, true));
    }
    else if(m_subType == MFVideoFormat_NV12)
    {
        DEBUG_MSG(L"NV12 frames %s \n", winrt::to_hstring(MFVideoFormat_NV12).data());

        DWORD frameBuffLen = m_width * m_height * 4;
        wil::unique_cotaskmem_ptr<BYTE[]> spBuff = wil::make_unique_cotaskmem_nothrow<BYTE[]>(frameBuffLen);
        RETURN_IF_NULL_ALLOC(spBuff.get());

        // NV12 is always top-down, so keep the conversion input top-down as well.
        RETURN_IF_FAILED(_CreateRGB32Frame(spBuff.get(), frameBuffLen, m_width * 4, m_width, m_height, rgbMask, false));
        RETURN_IF_FAILED(RGB32ToNV12Frame(spBuff.get(), frameBuffLen, m_width * 4, m_width, m_height, pBuf, len, pitch));
    }
    else
    {
        return MF_E_UNSUPPORTED_FORMAT;
    }

    return S_OK;
}

//////////////////////////////////////////////////
// private

HRESULT SimpleFrameGenerator::_CreateRGB32Frame(
    _Inout_updates_bytes_(len) BYTE* pBuf,
    _In_ DWORD len,
    _In_ LONG pitch,
    _In_ DWORD width,
    _In_ DWORD height,
    _In_ ULONG rgbMask,
    _In_ bool bottomUp)
{
    RETURN_HR_IF_NULL(E_INVALIDARG, pBuf);
    if (len < (abs(pitch) * height ))
    {
        return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
    }

    if (_ReadSharedRGB32Frame(pBuf, len, pitch, width, height, bottomUp))
    {
        return S_OK;
    }

    LONGLONG curSysTimeInS = MFGetSystemTime() / (MFTIME)10000000;
    int offset = curSysTimeInS % height;

    for (unsigned int r = 0; r < height; r++)
    {
        const DWORD destinationY = bottomUp ? height - 1 - r : r;
        BYTE* row = pitch >= 0
            ? pBuf + (destinationY * pitch)
            : pBuf + ((height - 1 - destinationY) * -pitch);
        uint32_t* p = reinterpret_cast<uint32_t*>(row);
        for (unsigned int c = 0; c < width; c++)
        {
            BYTE gray = (BYTE)(r + offset);
            *p = ((uint32_t)gray << 16 | (uint32_t)gray << 8 | (uint32_t)gray) & rgbMask;
            p++;
        }
    }

    return S_OK;
}

bool SimpleFrameGenerator::_ReadSharedRGB32Frame(
    BYTE* pBuf,
    DWORD len,
    LONG pitch,
    DWORD width,
    DWORD height,
    bool bottomUp)
{
    if (m_frameView == nullptr)
    {
        m_frameFile = CreateFileW(
            SHARED_FRAME_PATH,
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            nullptr,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            nullptr);
        if (m_frameFile == INVALID_HANDLE_VALUE)
        {
            return false;
        }

        m_frameMapping = CreateFileMappingW(
            m_frameFile,
            nullptr,
            PAGE_READWRITE,
            static_cast<DWORD>(SHARED_FRAME_FILE_SIZE >> 32),
            static_cast<DWORD>(SHARED_FRAME_FILE_SIZE),
            nullptr);
        if (m_frameMapping == nullptr)
        {
            _CloseSharedFrame();
            return false;
        }

        m_frameView = static_cast<BYTE*>(MapViewOfFile(
            m_frameMapping,
            FILE_MAP_READ | FILE_MAP_WRITE,
            0,
            0,
            static_cast<SIZE_T>(SHARED_FRAME_FILE_SIZE)));
        if (m_frameView == nullptr)
        {
            _CloseSharedFrame();
            return false;
        }
    }

    if (ReadU32(m_frameView, 0) != SHARED_FRAME_MAGIC ||
        ReadU32(m_frameView, 4) != SHARED_FRAME_VERSION ||
        ReadU32(m_frameView, 36) == 0)
    {
        return false;
    }

    const DWORD sourceWidth = ReadU32(m_frameView, 16);
    const DWORD sourceHeight = ReadU32(m_frameView, 20);
    const DWORD sourceStride = ReadU32(m_frameView, 24);
    const DWORD sourceLength = ReadU32(m_frameView, 28);
    if (sourceWidth == 0 || sourceHeight == 0 ||
        sourceWidth > SHARED_FRAME_MAX_WIDTH ||
        sourceHeight > SHARED_FRAME_MAX_HEIGHT ||
        sourceStride != sourceWidth * 4 ||
        sourceLength != sourceStride * sourceHeight ||
        sourceLength > SHARED_FRAME_FILE_SIZE - SHARED_FRAME_HEADER_SIZE ||
        len < static_cast<DWORD>(abs(pitch)) * height)
    {
        return false;
    }

    for (int attempt = 0; attempt < 3; ++attempt)
    {
        const LONGLONG before = ReadSequence(m_frameView);
        if ((before & 1) != 0)
        {
            YieldProcessor();
            continue;
        }

        const BYTE* source = m_frameView + SHARED_FRAME_HEADER_SIZE;
        for (DWORD y = 0; y < height; ++y)
        {
            const DWORD sourceY = (static_cast<ULONGLONG>(y) * sourceHeight) / height;
            const BYTE* sourceRow = source + sourceY * sourceStride;
            const DWORD destinationY = bottomUp ? height - 1 - y : y;
            BYTE* destinationRow = pitch >= 0
                ? pBuf + destinationY * pitch
                : pBuf + (height - 1 - destinationY) * -pitch;

            if (sourceWidth == width)
            {
                memcpy(destinationRow, sourceRow, width * 4);
            }
            else
            {
                auto* destinationPixel = reinterpret_cast<DWORD*>(destinationRow);
                const auto* sourcePixel = reinterpret_cast<const DWORD*>(sourceRow);
                for (DWORD x = 0; x < width; ++x)
                {
                    const DWORD sourceX = (static_cast<ULONGLONG>(x) * sourceWidth) / width;
                    destinationPixel[x] = sourcePixel[sourceX];
                }
            }
        }

        MemoryBarrier();
        const LONGLONG after = ReadSequence(m_frameView);
        if (before == after && (after & 1) == 0)
        {
            InterlockedExchange64(
                reinterpret_cast<volatile LONGLONG*>(m_frameView + 40),
                static_cast<LONGLONG>(GetTickCount64()));
            return true;
        }
    }

    return false;
}

void SimpleFrameGenerator::_CloseSharedFrame()
{
    if (m_frameView != nullptr)
    {
        UnmapViewOfFile(m_frameView);
        m_frameView = nullptr;
    }
    if (m_frameMapping != nullptr)
    {
        CloseHandle(m_frameMapping);
        m_frameMapping = nullptr;
    }
    if (m_frameFile != INVALID_HANDLE_VALUE)
    {
        CloseHandle(m_frameFile);
        m_frameFile = INVALID_HANDLE_VALUE;
    }
}

//////////////////////////////////////////////////
// pixelFormatConverter

void SimpleFrameGenerator::RGB24ToYUY2(int R, int G, int B, BYTE* pY, BYTE* pU, BYTE* pV)
{
    *pY = ((66 * R + 129 * G + 25 * B + 128) >> 8) + 16;
    *pU = ((-38 * R - 74 * G + 112 * B + 128) >> 8) + 128;
    *pV = ((112 * R - 94 * G - 18 * B + 128) >> 8) + 128;
}

void SimpleFrameGenerator::RGB24ToY(int R, int G, int B, BYTE* pY)
{
    *pY = ((66 * R + 129 * G + 25 * B + 128) >> 8) + 16;
}

void SimpleFrameGenerator::RGB32ToNV12(BYTE RGB1[8], BYTE RGB2[8], BYTE* pY1, BYTE* pY2, BYTE* pUV)
{
    RGB24ToYUY2(RGB1[2], RGB1[1], RGB1[0], pY1, pUV, pUV + 1);
    RGB24ToY(RGB1[6], RGB1[5], RGB1[4], pY1 + 1);
    RGB24ToYUY2(RGB2[2], RGB2[1], RGB2[0], pY2, pUV, pUV + 1);
    RGB24ToY(RGB2[6], RGB2[5], RGB2[4], pY2 + 1);
};

//////////////////////////////////////////////////
// FrameFormatConverter

HRESULT SimpleFrameGenerator::RGB32ToNV12Frame(_Inout_updates_bytes_(len) BYTE* pbBuff, ULONG cbBuff, long stride, UINT width, UINT height, BYTE* pbBuffOut, ULONG cbBuffOut, long strideOut)
{
    do
    {
        RETURN_HR_IF(E_UNEXPECTED, width * 4 * height > cbBuff);
        RETURN_HR_IF(E_UNEXPECTED, width * 1.5 * height > cbBuffOut);
        RETURN_HR_IF_NULL(E_INVALIDARG, pbBuff);

        RETURN_HR_IF_NULL(E_INVALIDARG, pbBuffOut);
        for (DWORD h = 0; h < height - 1; h += 2)
        {
            BYTE* pRGB1 = h * stride + pbBuff;
            BYTE* pRGB2 = (h + 1) * stride + pbBuff;
            BYTE* pY1 = h * strideOut + pbBuffOut;
            BYTE* pY2 = (h + 1) * strideOut + pbBuffOut;
            BYTE* pUV = (h / 2 + height) * strideOut + pbBuffOut;

            for (DWORD w = 0; w < width; w += 2)
            {
                RGB32ToNV12(pRGB1, pRGB2, pY1, pY2, pUV);
                pRGB1 += 8;
                pRGB2 += 8;
                pY1 += 2;
                pY2 += 2;
                pUV += 2;
            }
        }
    } while (FALSE);

    return S_OK;
}
