//! Codex 日志的存储格式边界：上层始终读取解压后的字节，元数据仍描述磁盘文件。
use std::fs::{File, Metadata};
use std::io::{self, BufReader, Error, ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

enum Input {
    Plain(File),
    Zstd(zstd::stream::read::Decoder<'static, BufReader<File>>),
}

pub(super) struct Reader {
    metadata: Metadata,
    input: Input,
}

impl Reader {
    pub fn open(path: &Path) -> io::Result<Self> {
        // 继续由文件描述符验证普通文件，保留 FIFO 非阻塞保护和正常符号链接支持。
        let file = super::super::open_jsonl(path)?;
        let metadata = file.metadata()?;
        let input = if path.extension().is_some_and(|ext| ext == "zst") {
            Input::Zstd(zstd::stream::read::Decoder::new(file)?)
        } else {
            Input::Plain(file)
        };
        Ok(Self { metadata, input })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// 普通文件可以直接确定正文长度；压缩流需完整读取后才能确认，首屏不为此解压全文件。
    pub fn decoded_size_hint(&self) -> Option<u64> {
        matches!(self.input, Input::Plain(_)).then(|| self.metadata.len())
    }

    /// 跳过当前解压流的前缀；工具引用在新打开的流上调用，不能对压缩文件直接 seek。
    pub fn skip_prefix(&mut self, bytes: u64) -> io::Result<()> {
        match &mut self.input {
            Input::Plain(reader) => {
                let end = reader
                    .stream_position()?
                    .checked_add(bytes)
                    .filter(|end| *end <= self.metadata.len())
                    .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "会话文件已截断"))?;
                reader.seek(SeekFrom::Start(end))?;
            }
            Input::Zstd(reader) => {
                let skipped = io::copy(&mut reader.take(bytes), &mut io::sink())?;
                if skipped != bytes {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof,
                        "会话解压后的内容已截断",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.input {
            Input::Plain(reader) => reader.read(buffer),
            Input::Zstd(reader) => reader.read(buffer),
        }
    }
}
