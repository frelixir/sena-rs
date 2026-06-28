cmake_minimum_required(VERSION 3.22.1)

project(sena_jni)

add_library(sena_jni SHARED
        sena_jni.cpp
)

find_library(android-lib android)
find_library(log-lib log)
find_library(dl-lib dl)

target_compile_features(sena_jni PRIVATE cxx_std_17)
target_link_libraries(sena_jni
        ${android-lib}
        ${log-lib}
        ${dl-lib}
)
