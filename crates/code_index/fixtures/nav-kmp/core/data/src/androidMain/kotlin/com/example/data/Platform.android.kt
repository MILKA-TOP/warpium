package com.example.data

actual fun platformName(): String = "Android"

fun androidGreeting(): String = "Hi from " + platformName()
