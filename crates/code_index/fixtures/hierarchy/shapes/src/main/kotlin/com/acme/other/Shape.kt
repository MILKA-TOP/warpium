package com.acme.other

interface Shape {
    fun area(): Double
}

class Blob : Shape {
    override fun area(): Double = 0.0
}
