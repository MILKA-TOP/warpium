package com.acme.shapes

interface Shape {
    fun area(): Double
    val name: String
}

abstract class Polygon : Shape {
    abstract fun sides(): Int
}
