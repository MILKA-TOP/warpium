package com.acme.shapes.impl

import com.acme.shapes.Polygon

class Square(override val name: String) : Polygon() {
    override fun area(): Double = 1.0
    override fun sides(): Int = 4
    fun area(scale: Double): Double = scale
}
