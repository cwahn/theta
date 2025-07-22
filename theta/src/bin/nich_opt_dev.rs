use std::any::Any;
use std::mem::size_of;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use uuid::Uuid;

// Simulated types for demonstration
type MsgPack = String; // Placeholder
trait Actor: Send + 'static {}
struct DummyActor;
impl Actor for DummyActor {}

pub type MsgTx = UnboundedSender<MsgPack>;
pub type Continuation = oneshot::Sender<Box<dyn Any + Send>>;

// Your current structure (probably 32 bytes)
struct NewActorRef {
    id: Uuid,          // 16 bytes
    tx: Option<MsgTx>, // 8 bytes (with niche optimization)
}

enum NewContinuation {
    Reply(Continuation),   // 8 bytes
    ActorRef(NewActorRef), // 24 bytes
    None,                  // 0 bytes (but enum discriminant adds padding)
}

// SOLUTION 1: Manual tagged union with raw pointers
#[repr(C)]
struct OptimizedContinuation {
    // Use the lower 2 bits of the pointer as a tag
    // 00 = None, 01 = Reply, 10 = ActorRef
    tagged_ptr: *mut u8,
    data: [u8; 16], // Store either Uuid or the rest of oneshot::Sender
}

impl OptimizedContinuation {
    const TAG_MASK: usize = 0b11;
    const TAG_NONE: usize = 0b00;
    const TAG_REPLY: usize = 0b01;
    const TAG_ACTOR_REF: usize = 0b10;

    pub fn none() -> Self {
        Self {
            tagged_ptr: Self::TAG_NONE as *mut u8,
            data: [0; 16],
        }
    }

    pub fn reply(sender: Continuation) -> Self {
        // Store the oneshot::Sender in the data field
        // This is unsafe and would need careful implementation
        let ptr = Box::into_raw(Box::new(sender)) as *mut u8;
        let tagged = (ptr as usize | Self::TAG_REPLY) as *mut u8;

        Self {
            tagged_ptr: tagged,
            data: [0; 16],
        }
    }

    pub fn actor_ref(id: Uuid, tx: Option<MsgTx>) -> Self {
        // Store UUID in data field and tx in tagged_ptr
        let mut data = [0u8; 16];
        data.copy_from_slice(id.as_bytes());

        let tx_ptr = match tx {
            Some(sender) => Box::into_raw(Box::new(sender)) as *mut u8,
            None => std::ptr::null_mut(),
        };
        let tagged = (tx_ptr as usize | Self::TAG_ACTOR_REF) as *mut u8;

        Self {
            tagged_ptr: tagged,
            data,
        }
    }

    pub fn tag(&self) -> usize {
        (self.tagged_ptr as usize) & Self::TAG_MASK
    }

    pub fn is_none(&self) -> bool {
        self.tag() == Self::TAG_NONE
    }
}

// SOLUTION 2: Using NonNull for better niche optimization
use std::ptr::NonNull;

#[derive(Debug)]
enum CompactContinuation {
    None,
    Reply(NonNull<oneshot::Sender<Box<dyn Any + Send>>>),
    ActorRef {
        id: Uuid,                   // 16 bytes
        tx: Option<NonNull<MsgTx>>, // 8 bytes with niche optimization
    },
}

impl CompactContinuation {
    pub fn none() -> Self {
        Self::None
    }

    pub fn reply(sender: Continuation) -> Self {
        let boxed = Box::new(sender);
        Self::Reply(NonNull::from(Box::leak(boxed)))
    }

    pub fn actor_ref(id: Uuid, tx: Option<MsgTx>) -> Self {
        let tx_ptr = tx.map(|sender| {
            let boxed = Box::new(sender);
            NonNull::from(Box::leak(boxed))
        });

        Self::ActorRef { id, tx: tx_ptr }
    }
}

// SOLUTION 3: Pack everything into 24 bytes with bit manipulation
#[repr(C, packed)]
struct PackedContinuation {
    // Byte 0-1: Tag (2 bytes for alignment, only need 2 bits)
    tag: u16,
    // Byte 2-17: UUID or pointer data (16 bytes)
    data: [u8; 16],
    // Byte 18-23: Additional pointer or data (6 bytes)
    extra: [u8; 6],
}

impl PackedContinuation {
    const TAG_NONE: u16 = 0;
    const TAG_REPLY: u16 = 1;
    const TAG_ACTOR_REF: u16 = 2;

    pub fn none() -> Self {
        Self {
            tag: Self::TAG_NONE,
            data: [0; 16],
            extra: [0; 6],
        }
    }

    // Implementation would store oneshot::Sender across data + extra
    pub fn reply(sender: Continuation) -> Self {
        // This would require unsafe manipulation of the sender
        Self {
            tag: Self::TAG_REPLY,
            data: [0; 16], // Would store sender data here
            extra: [0; 6],
        }
    }

    pub fn actor_ref(id: Uuid, tx_present: bool) -> Self {
        let mut data = [0u8; 16];
        data.copy_from_slice(id.as_bytes());

        let mut extra = [0u8; 6];
        if tx_present {
            extra[0] = 1; // Flag indicating tx is present
            // Would store tx pointer in remaining bytes
        }

        Self {
            tag: Self::TAG_ACTOR_REF,
            data,
            extra,
        }
    }
}

// SOLUTION 4: Union-based approach (unsafe but explicit)
#[repr(C)]
union ContinuationData {
    reply: std::mem::ManuallyDrop<oneshot::Sender<Box<dyn Any + Send>>>,
    actor_ref: std::mem::ManuallyDrop<ActorRefData>,
}

#[repr(C)]
struct ActorRefData {
    id: Uuid, // 16 bytes
    has_tx: bool, // 1 byte
              // tx would be stored separately or as a pointer
}

#[repr(C)]
struct UnionContinuation {
    tag: u8,                     // 1 byte
    data: ContinuationData,      // Should be 16 bytes if properly optimized
    tx_ptr: Option<NonNull<()>>, // 8 bytes, stores MsgTx when needed
}

fn main() {
    println!("Size analysis:");
    println!("Uuid: {} bytes", size_of::<Uuid>());
    println!(
        "oneshot::Sender<Box<dyn Any + Send>>: {} bytes",
        size_of::<oneshot::Sender<Box<dyn Any + Send>>>()
    );
    println!(
        "Option<oneshot::Sender<Box<dyn Any + Send>>>: {} bytes",
        size_of::<Option<oneshot::Sender<Box<dyn Any + Send>>>>()
    );
    println!(
        "UnboundedSender<String>: {} bytes",
        size_of::<UnboundedSender<String>>()
    );
    println!(
        "Option<UnboundedSender<String>>: {} bytes",
        size_of::<Option<UnboundedSender<String>>>()
    );

    println!("\nYour current structures:");
    println!("NewActorRef: {} bytes", size_of::<NewActorRef>());
    println!("NewContinuation: {} bytes", size_of::<NewContinuation>());

    println!("\nOptimized structures:");
    println!(
        "OptimizedContinuation: {} bytes",
        size_of::<OptimizedContinuation>()
    );
    println!(
        "CompactContinuation: {} bytes",
        size_of::<CompactContinuation>()
    );
    println!(
        "PackedContinuation: {} bytes",
        size_of::<PackedContinuation>()
    );
    println!(
        "UnionContinuation: {} bytes",
        size_of::<UnionContinuation>()
    );

    // Pointer alignment info
    println!("\nPointer alignment:");
    println!("Pointer size: {} bytes", size_of::<*mut u8>());
    println!(
        "Pointer alignment: {} bytes",
        std::mem::align_of::<*mut u8>()
    );
}
