use super::*;

#[test]
fn every_tab_has_its_own_filter() {
    for tab in SearchTab::ALL {
        assert_eq!(filter_tab(tab_filter(tab)), Some(tab));
    }
    assert_eq!(tab_filter(SearchTab::Files), QueryFilter::Files);
    assert_eq!(tab_filter(SearchTab::Classes), QueryFilter::CodeClasses);
}

#[test]
fn other_filters_are_no_tab() {
    assert_eq!(filter_tab(QueryFilter::Actions), None);
    assert_eq!(filter_tab(QueryFilter::CodeNavigation), None);
}
