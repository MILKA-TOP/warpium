package com.example.shared

actual class Platform actual constructor() {
    actual val name: String = "Android"
}

actual fun platformName(): String = "Android"
