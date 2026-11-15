package com.example.feature.news

// getNewsResources is called by the view model, not here.
fun newsScreen(viewModel: NewsViewModel): String {
    val size = NewsViewModel.PAGE_SIZE
    viewModel.load()
    return "getNewsResources $size"
}
