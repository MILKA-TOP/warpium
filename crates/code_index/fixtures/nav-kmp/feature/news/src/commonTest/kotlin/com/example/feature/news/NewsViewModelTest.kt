package com.example.feature.news

import com.example.data.test.FakeNewsRepository

class NewsViewModelTest {
    fun loads() {
        val repository = FakeNewsRepository()
        repository.getNewsResources()
        NewsViewModel(repository).load()
    }
}
