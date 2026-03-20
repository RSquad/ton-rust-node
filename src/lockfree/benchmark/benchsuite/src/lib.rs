pub mod exec;
pub mod report;

#[macro_export]
macro_rules! bench {
    (levels $($level:expr),*; $($name:expr => $target:expr),*) => {{
        let res = $crate::report::write(
            &mut ::std::io::stdout(),
            &($($target),*),
            &[$($level,)*],
            &[$($name,)*]
        );
        res.expect("fatal error on writing to stdout")
    }};

    (levels $($level:expr,)*; $($tok:tt)*) => {
        bench!(levels $($level),*; $($tok)*);
    };

    (levels $($level:expr),*; $($name:expr => $target:expr,)*) => {
        bench!(levels $($level),*; $($name => $target),*);
    };
}
