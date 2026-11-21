# SPDX-License-Identifier: GPL-3.0-or-later
cmake_minimum_required(VERSION 3.20)
project(mdk-xocn LANGUAGES CXX)

if(NOT WIN32 OR NOT MSVC OR NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
    message(FATAL_ERROR "The Sony X-OCN reader uses the Windows x64 Sony RAW SDK ABI.")
endif()

set(MDK_SDK "" CACHE PATH "MDK SDK with its matching ABI headers installed")
list(APPEND CMAKE_MODULE_PATH "${MDK_SDK}/lib/cmake")
find_package(MDK REQUIRED)
setup_mdk_plugin(mdk-xocn)
target_sources(mdk-xocn PRIVATE Reader.cpp mdk-xocn.def)
target_compile_features(mdk-xocn PRIVATE cxx_std_23)
target_compile_definitions(mdk-xocn PRIVATE NOMINMAX WIN32_LEAN_AND_MEAN)
