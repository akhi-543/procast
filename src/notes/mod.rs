// Task 19 renders the note with the rest of the editor; until then the app uses only `Buffer`.
#[cfg_attr(not(test), expect(dead_code, reason = "Task 19's editor rendering uses the rest"))]
pub mod editor;
pub mod store;
pub mod window;
