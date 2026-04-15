import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import type { ChatMessage, RoomInfo } from "@/hooks/use-chat";
import type { ChatRoomRef } from "theta:actors";

interface ChatViewProps {
  role: "host" | "guest";
  peerKey: string;
  messages: ChatMessage[];
  rooms: RoomInfo[];
  currentRoom: ChatRoomRef | null;
  onSend: (text: string) => void;
  onCreateRoom: (name: string) => void;
  onSelectRoom: (info: RoomInfo) => void;
  author: string;
}

export function ChatView({ role, peerKey, messages, rooms, currentRoom, onSend, onCreateRoom, onSelectRoom, author }: ChatViewProps) {
  const [text, setText] = useState("");
  const [newRoomName, setNewRoomName] = useState("");
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const handleSend = () => {
    const trimmed = text.trim();
    if (!trimmed) return;
    onSend(trimmed);
    setText("");
  };

  const handleCreateRoom = () => {
    const name = newRoomName.trim() || `room-${rooms.length + 1}`;
    onCreateRoom(name);
    setNewRoomName("");
  };

  const copyKey = async () => {
    await navigator.clipboard.writeText(peerKey);
  };

  return (
    <div className="flex-1 flex">
      {/* Room sidebar */}
      <div className="w-56 border-r flex flex-col">
        <div className="px-4 py-3 border-b">
          <span className="text-sm font-medium">Rooms ({rooms.length})</span>
        </div>
        <ScrollArea className="flex-1">
          <div className="p-2 space-y-1">
            {rooms.map((info) => (
              <button
                key={info.room.id}
                className={`w-full text-left px-3 py-2 rounded-md text-sm transition-colors ${
                  currentRoom?.id === info.room.id
                    ? "bg-accent text-accent-foreground"
                    : "hover:bg-muted"
                }`}
                onClick={() => onSelectRoom(info)}
              >
                # {info.name}
              </button>
            ))}
          </div>
        </ScrollArea>
        <div className="p-2 border-t">
          <div className="flex gap-1">
            <Input
              className="text-xs h-8"
              placeholder="New room..."
              value={newRoomName}
              onChange={(e) => setNewRoomName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleCreateRoom()}
            />
            <Button size="sm" variant="ghost" className="h-8 px-2" onClick={handleCreateRoom}>
              +
            </Button>
          </div>
        </div>
      </div>

      {/* Chat area */}
      <div className="flex-1 flex flex-col">
        <div className="border-b px-6 py-3 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Badge variant={role === "host" ? "default" : "secondary"}>
              {role === "host" ? "Hosting" : "Connected"}
            </Badge>
            <span
              className="text-xs text-muted-foreground font-mono cursor-pointer hover:text-foreground transition-colors"
              title="Click to copy"
              onClick={copyKey}
            >
              {peerKey.slice(0, 16)}...
            </span>
          </div>
          <span className="text-xs text-muted-foreground">Monitoring</span>
        </div>

        <ScrollArea className="flex-1 p-6">
          <div className="space-y-3">
            {messages.map((msg, i) => {
              const isMe = msg.author === author;
              const date = new Date(msg.timestamp * 1000);
              return (
                <div
                  key={i}
                  className={`rounded-lg border bg-card p-3 max-w-[80%] ${isMe ? "ml-auto" : ""}`}
                >
                  <div className="flex items-baseline gap-2 mb-1">
                    <span className={`text-sm font-medium ${isMe ? "text-primary" : "text-muted-foreground"}`}>
                      {msg.author}
                    </span>
                    <span className="text-xs text-muted-foreground">
                      {date.toLocaleTimeString()}
                    </span>
                  </div>
                  <p className="text-sm">{msg.text}</p>
                </div>
              );
            })}
            <div ref={bottomRef} />
          </div>
        </ScrollArea>

        <div className="border-t p-4 flex gap-3">
          <Input
            className="flex-1"
            placeholder="Type a message..."
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleSend()}
          />
          <Button onClick={handleSend}>Send</Button>
        </div>
      </div>
    </div>
  );
}
