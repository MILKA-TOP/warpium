package com.acme.app

import com.acme.shapes.Circle
import com.acme.shapes.Shape
import com.acme.shapes.impl.Square

fun shapeArea(shape: Shape): Double = shape.area()

fun circleArea(circle: Circle): Double = circle.area()

fun squareArea(): Double = Square("s").area()
