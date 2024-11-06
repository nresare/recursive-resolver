#[macro_export]
macro_rules! ns {
    ($name:expr, $target:expr) => {
        Record::from_rdata($name.parse()?, 0, RData::NS(rdata::NS($target.parse()?)))
    };
}

#[macro_export]
macro_rules! a {
    ($name:expr, $target:expr) => {
        Record::from_rdata($name.parse()?, 0, RData::A(rdata::A(($target.parse()?))))
    };
}

#[macro_export]
macro_rules! refer {
    ($nameservers:expr) => {{
        let mut msg = Message::new();
        msg.insert_name_servers(vec![$nameservers]);
        msg
    }};
    ($nameservers:expr, $glue:expr) => {{
        let mut msg = Message::new();
        msg.insert_name_servers(vec![$nameservers]);
        msg.insert_additionals(vec![$glue]);
        msg
    }};
}

#[macro_export]
macro_rules! answer {
    ($record:expr) => {{
        let mut msg = Message::new();
        let mut header = Header::default();
        header.set_authoritative(true);
        msg.set_header(header);
        msg.insert_answers(vec![$record]);
        msg
    }};
}
