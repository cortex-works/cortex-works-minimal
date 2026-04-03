#[macro_export]
macro_rules! debug_log {
	($($arg:tt)*) => {{
		#[cfg(debug_assertions)]
		{
			eprintln!($($arg)*);
		}
	}};
}


pub mod chronos;
pub mod config;
pub mod grammar_manager;
pub mod inspector;
pub mod mapper;
pub mod scanner;
pub mod skeleton;
pub mod tool_schemas;
pub mod server;
pub mod slicer;
pub mod universal;
pub mod workspace;
pub mod xml_builder;
pub mod z4_tools;
