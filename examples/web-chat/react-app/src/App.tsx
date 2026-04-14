import { useRef } from "react";
import { useChat } from "@/hooks/use-chat";
import { Landing } from "@/components/Landing";
import { ChatView } from "@/components/ChatView";

export function App() {
  const chat = useChat();
  const authorRef = useRef("");

  if (chat.status !== "connected") {
    return (
      <div className="min-h-screen flex flex-col bg-background text-foreground">
        <header className="border-b px-6 py-4">
          <h1 className="text-xl font-semibold tracking-tight">Theta Web Chat</h1>
          <p className="text-sm text-muted-foreground">React + vite-plugin-theta + typed actor API</p>
        </header>
        <Landing
          myKey={chat.myKey}
          status={chat.status}
          onCreateRoom={(name) => {
            authorRef.current = name;
            chat.createRoom();
          }}
          onJoinRoom={(name, key) => {
            authorRef.current = name;
            chat.joinRoom(key);
          }}
        />
        {chat.error && (
          <p className="text-sm text-destructive text-center pb-4">
            Error: {chat.error}
          </p>
        )}
      </div>
    );
  }

  return (
    <div className="min-h-screen flex flex-col bg-background text-foreground">
      <header className="border-b px-6 py-4">
        <h1 className="text-xl font-semibold tracking-tight">Theta Web Chat</h1>
        <p className="text-sm text-muted-foreground">React + vite-plugin-theta + typed actor API</p>
      </header>
      <ChatView
        role={chat.role!}
        peerKey={chat.peerKey}
        messages={chat.messages}
        rooms={chat.rooms}
        currentRoom={chat.currentRoom}
        author={authorRef.current}
        onSend={(text) => chat.sendMessage(authorRef.current, text)}
        onCreateRoom={(name) => chat.createRoom(name)}
        onSelectRoom={chat.selectRoom}
      />
    </div>
  );
}
