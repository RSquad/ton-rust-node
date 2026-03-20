use exec::{Executor, TargetSet};
use std::io::{self, Write};

pub fn write<W, T>(
    writer: &mut W,
    target_set: &T,
    thread_config: &[usize],
    names: &[&str],
) -> io::Result<()>
where
    W: Write,
    T: TargetSet,
{
    for &nthreads in thread_config {
        let mut tester = Executor::new(nthreads);
        target_set.run(&mut tester);
        write!(writer, "Result for {} threads:\n", nthreads)?;
        for (i, stat) in tester.stats().iter().enumerate() {
            write!(writer, "Target {} ", i)?;
            if let Some(name) = names.get(i) {
                write!(writer, "({})", name)?;
            }
            write!(writer, ":\n{}\n", stat)?;
        }
        write!(writer, "\n")?;
    }
    Ok(())
}
